#!/usr/bin/env python3
"""Scope fix-ups for migrated registrations (STORY-1728): qualify `cli.rs`
types in generated `*Args` structs, and give each generated `run_*` fn the
crate-root name resolution its body had inside `run()`.
trace:STORY-1728 | ai:claude
"""
import os, re, sys
SRC = os.path.join(os.path.dirname(__file__), "..", "..", "aida-cli-lib", "src")
cli = open(os.path.join(SRC, "cli.rs")).read()
cli_types = set(re.findall(r"^pub (?:enum|struct) (\w+)", cli, re.M))
files = sys.argv[1:] or [f for f in os.listdir(SRC) if f.endswith(".rs")]
for f in files:
    path = os.path.join(SRC, f)
    text = open(path).read()
    if "pub(crate) static " not in text or "crate::command_registry::CommandReg" not in text:
        continue
    lines = text.split("\n")
    out = []
    in_struct = False
    has_glob = re.search(r"^use crate::\*;", text, re.M) is not None
    for i, l in enumerate(lines):
        if re.match(r"^pub\(crate\) struct \w+Args \{", l) and i > 0 and any(
            "#[derive(clap::Parser, Debug)]" in x for x in lines[max(0, i - 6) : i]
        ):
            in_struct = True
        elif in_struct and l == "}":
            in_struct = False
        elif in_struct and re.match(r"^    pub (r#)?\w+: ", l):
            name, ty = l.split(": ", 1)
            ty = re.sub(
                r"(?<![\w:])(" + "|".join(sorted(cli_types, key=len, reverse=True)) + r")\b",
                r"crate::cli::\1",
                ty,
            )
            l = name + ": " + ty
        out.append(l)
        if (
            not has_glob
            and re.match(r"^fn run_\w+\(_?matches: &clap::ArgMatches, _?ctx: &crate::command_registry::DispatchCtx\) -> anyhow::Result<\(\)> \{$", l)
            and (i + 1 >= len(lines) or lines[i + 1].strip() != "use crate::*;")
        ):
            out.append("    use crate::*;")
    new = "\n".join(out)
    if new != text:
        open(path, "w").write(new)
        print("fixed", f)
