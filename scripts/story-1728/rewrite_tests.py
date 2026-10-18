#!/usr/bin/env python3
"""Rewrite test code that matches on migrated `Command::<Variant>` patterns
(STORY-1728): parse through the registry, and match the command's typed args.

    python3 scripts/story-1728/rewrite_tests.py [files...]

`Command::V { .. }` -> `<mod>::VArgs { .. }`, `Command::V(P)` ->
`<mod>::VArgs { inner: P }`, and the scrutinee of the enclosing `matches!`,
`match` or `let ... else` gains `.registered::<<mod>::VArgs>()`.
trace:STORY-1728 | ai:claude
"""
import os, re, sys
SRC = os.path.join(os.path.dirname(__file__), "..", "..", "aida-cli-lib", "src")

def registrations():
    regs = {}
    for f in os.listdir(SRC):
        if not f.endswith(".rs"):
            continue
        t = open(os.path.join(SRC, f)).read()
        for m in re.finditer(r"command: <(\w+) as clap::CommandFactory>::command", t):
            ty = m.group(1)
            v = re.sub(r"(Cmd)?Args$", "", ty)
            regs[v] = f"crate::{f[:-3]}::{ty}"
    return regs

def close_of(s, i):
    """Index of the bracket closing the one at s[i]."""
    pairs = {"(": ")", "{": "}", "[": "]"}
    stack = []
    j = i
    while j < len(s):
        c = s[j]
        if c == '"':
            j += 1
            while s[j] != '"':
                j += 2 if s[j] == "\\" else 1
        elif c in pairs:
            stack.append(pairs[c])
        elif stack and c == stack[-1]:
            stack.pop()
            if not stack:
                return j
        j += 1
    raise ValueError("unbalanced")

def rewrite(text, regs):
    text = re.sub(r"\b(?:crate::cli::|crate::|cli::)?Cli::try_parse_from\(", "crate::command_registry::try_parse_from(", text)
    changed = True
    while changed:
        changed = False
        for v, path in regs.items():
            m = re.search(r"(?<![\w:])(?:crate::cli::|cli::)?Command::" + v + r"\b", text)
            if not m:
                continue
            a, b = m.start(), m.end()
            rest = text[b:]
            if rest.startswith("("):
                c = close_of(text, b)
                inner = text[b + 1 : c]
                new = f"{path} {{ inner: {inner} }}"
                end = c + 1
            elif rest.lstrip().startswith("{"):
                o = b + (len(rest) - len(rest.lstrip()))
                c = close_of(text, o)
                new = path + " " + text[o : c + 1]
                end = c + 1
            else:
                new = path + " {}"
                end = b
            # scrutinee: the nearest enclosing construct before the pattern
            before = text[:a]
            k_m = before.rfind("matches!(")
            k_match = max(before.rfind("match "), before.rfind("match\n"))
            k_let = before.rfind("let ")
            k = max(k_m, k_match, k_let)
            text = text[:a] + new + text[end:]
            if k == k_m:
                s0 = k + len("matches!(")
                comma = s0
                depth = 0
                while True:
                    ch = text[comma]
                    if ch in "([{":
                        depth += 1
                    elif ch in ")]}":
                        depth -= 1
                    elif ch == "," and depth == 0:
                        break
                    comma += 1
                expr = text[s0:comma].rstrip()
                text = text[:s0] + expr + f".registered::<{path}>()" + text[s0 + len(expr):]
            elif k == k_match:
                s0 = k + len("match ")
                brace = text.index("{", s0)
                expr = text[s0:brace].rstrip()
                text = text[:s0] + expr + f".registered::<{path}>()" + text[s0 + len(expr):]
            else:
                # let PAT = EXPR else
                eq = text.index("=", a + len(new))
                el = text.index("else", eq)
                expr = text[eq + 1 : el].rstrip()
                text = text[: eq + 1] + expr + f".registered::<{path}>()" + text[eq + 1 + len(expr):]
            changed = True
    return text

regs = registrations()
for f in sys.argv[1:]:
    p = f if os.path.isabs(f) else os.path.join(SRC, f)
    t = open(p).read()
    n = rewrite(t, regs)
    if n != t:
        open(p, "w").write(n)
        print("rewrote", f)
