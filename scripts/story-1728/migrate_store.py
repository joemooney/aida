#!/usr/bin/env python3
"""Move one storage-dispatched CLI command into the command registry as a
`Handler::Store` registration (STORY-1728).

    python3 scripts/story-1728/migrate_store.py <Variant> --module <mod> [--dry-run]

Collects, for the variant:
  * its early `if let Command::V… = &cli.command { … }` blocks in `run()`
    (zero or more, in order), which run with no store;
  * its arms in the git-backend dispatcher (`handle_git_backend_command`);
  * its arms in the centralized dispatcher in `run()`;
and emits one store-phase handler that branches on `ctx.store`. Each arm's
`return x;` becomes `return returned(x);` so the dispatcher still sees
Return vs fall-through. A variant with no git-backend arm keeps the git
dispatcher's "not yet supported" exit. Arms shared with other variants are
reported, not moved.
trace:STORY-1728 | ai:claude
"""
import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(__file__))
import migrate_command as mc  # noqa: E402

SRC = mc.SRC


def depth_scan(text, i, stop):
    """Index of the first char in `stop` at bracket depth 0 from i."""
    depth = 0
    j = i
    while j < len(text):
        c = text[j]
        if c == '"':
            j += 1
            while text[j] != '"':
                j += 2 if text[j] == "\\" else 1
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", text[j : j + 4]):
            j = text.index("'", j + 1)
        elif text.startswith("//", j):
            j = text.index("\n", j)
            continue
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            if depth == 0 and c in stop:
                return j
            depth -= 1
        elif depth == 0 and any(text.startswith(s, j) for s in stop if len(s) > 1 or s not in ")]}"):
            return j
        j += 1
    raise ValueError("scan ran off the end")


def match_bounds(text, header_re, after=None):
    """(start, end) of the body of the `match … {` found by header_re, at or
    after the `after` anchor text when given."""
    base = text.index(after) if after else 0
    m = re.compile(header_re, re.M).search(text, base)
    if not m:
        raise SystemExit(f"dispatcher match not found: {header_re}")
    open_at = text.index("{", m.end() - 1)
    close = mc_close(text, open_at)
    return open_at + 1, close


def mc_close(text, i):
    depth = 0
    j = i
    while j < len(text):
        c = text[j]
        if c == '"':
            j += 1
            while text[j] != '"':
                j += 2 if text[j] == "\\" else 1
        elif text.startswith("//", j):
            j = text.index("\n", j)
            continue
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return j
        j += 1
    raise ValueError("unbalanced")


def arms(text, start, end, indent="        "):
    """Yield (arm_start, arm_end, pattern, body) for each arm in text[start:end]."""
    out = []
    pos = start
    line_re = re.compile(r"^" + indent + r"(?=[A-Za-z_(&])", re.M)
    while True:
        m = line_re.search(text, pos, end)
        if not m:
            break
        a = m.start()
        # leading comment lines
        lead = a
        while True:
            prev_nl = text.rfind("\n", 0, lead - 1)
            line = text[prev_nl + 1 : lead - 1] if lead > 0 else ""
            if line.strip().startswith("//"):
                lead = prev_nl + 1
            else:
                break
        arrow = depth_scan(text, a, ["=>"])
        pattern = text[a:arrow].strip()
        b = arrow + 2
        while text[b] in " \t\n":
            b += 1
        if text[b] == "{":
            c = mc_close(text, b)
            body_end = c + 1
            if text[body_end : body_end + 1] == ",":
                body_end += 1
            body = text[b : c + 1]
            block = True
        else:
            c = depth_scan(text, b, [",", "}"])
            body_end = c + 1 if text[c] == "," else c
            body = text[b:c]
            block = False
        out.append((lead, body_end, pattern, body, block))
        pos = body_end
    return out


def rewrite_pattern(pattern, variant, args_path):
    """`Command::V…` alternatives -> args-struct patterns, or None if the arm
    also matches other variants."""
    alts = [p.strip() for p in split_top(pattern, "|")]
    guard = ""
    last = alts[-1]
    gm = re.search(r"\s+if\s+", last)
    if gm and depth_ok(last, gm.start()):
        guard = last[gm.start():]
        alts[-1] = last[: gm.start()]
    out = []
    for alt in alts:
        m = re.match(r"(?:crate::cli::|cli::|crate::)?Command::" + variant + r"\b", alt)
        if not m:
            return None
        rest = alt[m.end():].strip()
        if rest.startswith("("):
            out.append(f"{args_path} {{ inner: {rest[1:-1].strip()} }}")
        elif rest.startswith("{"):
            out.append(f"{args_path} {rest}")
        elif rest == "":
            out.append(f"{args_path} {{}}")
        else:
            return None
    return " | ".join(out) + guard


def depth_ok(s, i):
    d = 0
    for c in s[:i]:
        d += c in "([{"
        d -= c in ")]}"
    return d == 0


def split_top(s, sep):
    parts, d, cur = [], 0, ""
    i = 0
    while i < len(s):
        c = s[i]
        if c in "([{":
            d += 1
        elif c in ")]}":
            d -= 1
        if c == sep and d == 0 and not s.startswith("||", i):
            parts.append(cur)
            cur = ""
        else:
            cur += c
        i += 1
    parts.append(cur)
    return parts


def code_mask(text):
    """True for each char that is code (not inside a string or comment)."""
    mask = [True] * len(text)
    j = 0
    while j < len(text):
        if text[j] == '"':
            k = j + 1
            while k < len(text) and text[k] != '"':
                k += 2 if text[k] == "\\" else 1
            for x in range(j, min(k + 1, len(text))):
                mask[x] = False
            j = k + 1
        elif text.startswith("//", j):
            k = text.find("\n", j)
            k = len(text) if k < 0 else k
            for x in range(j, k):
                mask[x] = False
            j = k
        else:
            j += 1
    return mask


def wrap_returns(body):
    """`return x;` -> `return crate::command_registry::returned(x);`."""
    out = ""
    pos = 0
    mask = code_mask(body)
    for m in re.finditer(r"\breturn\b", body):
        if m.start() < pos or not mask[m.start()]:
            continue
        start = m.end()
        semi = depth_scan(body, start, [";", "}"])
        expr = body[start:semi].strip()
        out += body[pos : m.start()] + f"return crate::command_registry::returned({expr})"
        pos = semi
    out += body[pos:]
    return out


def reindent(text, delta):
    lines = text.split("\n")
    res = []
    for l in lines:
        if delta > 0:
            res.append((" " * delta + l) if l.strip() else l)
        else:
            res.append(l[-delta:] if l.startswith(" " * -delta) else l.lstrip())
    return "\n".join(res)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("variant")
    ap.add_argument("--module", required=True)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    v = args.variant
    module = args.module
    names = ["cli.rs", "lib_part1.rs", "git_backend_cmd.rs", "stakeholder_role.rs", "role.rs", "command_registry.rs"]
    texts = {n: mc.read(n) for n in names}
    mod_file = module + ".rs"
    if mod_file not in texts:
        texts[mod_file] = mc.read(mod_file)
    problems = []

    legacy_before = mc.legacy_names(texts["cli.rs"])
    block, head_off, kind, texts["cli.rs"] = mc.cut_variant(texts["cli.rs"], v)
    cli_name, struct_lines, binding = mc.parser_struct(block, head_off, kind, v, texts["cli.rs"])
    args_ty = f"{v}Args"
    if re.search(r"\b(?:struct|enum|type) " + args_ty + r"\b", texts[mod_file]):
        args_ty = f"{v}CmdArgs"
        struct_lines = [l.replace(f"{v}Args", args_ty) for l in struct_lines]
    args_path = args_ty

    # early blocks
    lib = texts["lib_part1.rs"]
    lines = lib.split("\n")
    run_at = next(i for i, l in enumerate(lines) if l.startswith("pub(crate) fn run() -> Result<()> {"))
    storage_at = next(i for i, l in enumerate(lines) if l.strip() == "let storage = Storage::new(requirements_path.clone());")
    early = []
    i = run_at
    while i < storage_at:
        if re.match(r"^    if let Command::" + v + r"\b", lines[i]):
            j = i
            while "= &cli.command" not in lines[j]:
                j += 1
            pat_text = " ".join(l.strip() for l in lines[i : j + 1])
            body_open = j if lines[j].rstrip().endswith("{") else j + 1
            e = mc.brace_end(lines, body_open)
            st = mc.leading_comments(lines, i, include_attrs=False)
            m = re.match(r"if let (.*?)\s*= &cli\.command\s*\{?$", pat_text)
            pat = rewrite_pattern(m.group(1), v, args_path)
            if pat is None:
                raise SystemExit(f"early pattern not rewritable: {m.group(1)}")
            comments = [l[4:] for l in lines[st:i]]
            body = "\n".join(l[8:] if l.startswith("        ") else l.strip() for l in lines[body_open + 1 : e])
            early.append((comments, pat, body))
            stop = e + 1
            if stop < len(lines) and lines[stop].strip() == "" and lines[st - 1].strip() == "":
                stop += 1
            del lines[st:stop]
            storage_at -= stop - st
            i = st
            continue
        i += 1
    texts["lib_part1.rs"] = "\n".join(lines)

    def take_arms(name, header_re, after=None):
        t = texts[name]
        s, e = match_bounds(t, header_re, after)
        found = []
        for (a, b, pattern, body, block) in reversed(arms(t, s, e)):
            if not re.search(r"(?<![\w:])Command::" + v + r"\b", pattern):
                continue
            pat = rewrite_pattern(pattern, v, args_path)
            if pat is None:
                # Shared arm: split this variant's alternatives off into its
                # own copy of the arm and keep the rest in place.
                gm = re.search(r"\s+if\s+", pattern)
                if gm and depth_ok(pattern, gm.start()):
                    problems.append(f"{name}: shared arm with a guard left in place: {pattern[:80]}")
                    continue
                alts = [x.strip() for x in split_top(pattern, "|")]
                own = re.compile(r"(?:crate::cli::|cli::|crate::)?Command::" + v + r"\b")
                ours = [x for x in alts if own.match(x)]
                others = [x for x in alts if not own.match(x)]
                pat = rewrite_pattern(" | ".join(ours), v, args_path)
                if pat is None or not others:
                    problems.append(f"{name}: arm not rewritable: {pattern[:80]}")
                    continue
                p_at = t.index(pattern, a)
                t = t[:p_at] + " | ".join(others) + t[p_at + len(pattern):]
                found.insert(0, ("", pat, body, block))
                continue
            if re.fullmatch(r"\s*\{?\s*unreachable!\([^)]*\)[;,]?\s*\}?\s*", body, re.S):
                t = t[:a] + t[b:].lstrip(" ").lstrip("\n") if False else t[:a] + t[b:]
                continue
            comments = t[a : t.index(pattern, a)].rstrip()
            found.insert(0, (comments, pat, body, block))
            t = t[:a] + t[b:]
        texts[name] = t
        return found

    git_arms = take_arms(
        "git_backend_cmd.rs", r"^    match command \{", after="pub(crate) fn handle_git_backend_command("
    )
    legacy_arms = take_arms(
        "lib_part1.rs",
        r"^    match &cli\.command \{",
        after="    let storage = Storage::new(requirements_path.clone());",
    )
    git_arms_anchor = None

    stakeholder, push, skip, pproblems = mc.policy_for(v, texts)
    problems += pproblems
    mc.strip_from_tables(v, texts)
    for n in ("lib_part1.rs", "git_backend_cmd.rs"):
        texts[n], _ = mc.strip_unreachable_arms(texts[n], v)

    regs = mc.registered(SRC)
    order = mc.built_order(legacy_before, regs)
    idx = order.index(cli_name)
    help_after = order[idx - 1] if idx > 0 else ""
    static = mc.snake(v).upper()
    fn_name = f"run_{mc.snake(v)}"

    def arm_text(comments, pat, body, block):
        body = wrap_returns(body)
        body = body.replace(f"crate::{module}::", "self::")
        body = re.sub(r"(?<![\w:])" + module + r"::(?=\w+\()", "self::", body)
        c = reindent(comments.strip("\n"), -8 + 12) + "\n" if comments.strip() else ""
        return f"{c}            {pat} => {reindent(body, 4).lstrip() if block else body + ','}"

    out = []
    out.append(f"fn {fn_name}(matches: &clap::ArgMatches, ctx: &crate::command_registry::DispatchCtx) -> anyhow::Result<crate::command_registry::Flow> {{")
    out.append("    use crate::*;")
    out.append(f"    let args = <{args_ty} as clap::FromArgMatches>::from_arg_matches(matches)?;")
    out.append("    match &ctx.store {")
    out.append("        None => {")
    for comments, pat, body in early:
        for c in comments:
            out.append("            " + c if c.strip() else "")
        body = wrap_returns(body).replace("cli.globals", "ctx.globals.expect(\"globals at the early dispatch point\")")
        out.append(f"            if let {pat} = &args {{")
        out.append(reindent(body, 16))
        out.append("            }")
    out.append("            Ok(crate::command_registry::Flow::Continue)")
    out.append("        }")
    out.append("        Some(crate::command_registry::StoreCtx::Git { store_path, backend }) => {")
    out.append("            let (store_path, backend) = (*store_path, *backend);")
    if git_arms:
        out.append("            match &args {")
        for a in git_arms:
            out.append(arm_text(*a))
        out.append("                #[allow(unreachable_patterns)]")
        out.append("                _ => crate::git_backend_cmd::exit_unsupported_on_git_backend(),")
        out.append("            }")
        out.append("            Ok(crate::command_registry::Flow::Continue)")
    else:
        out.append("            let _ = (store_path, backend, &args);")
        out.append("            crate::git_backend_cmd::exit_unsupported_on_git_backend()")
    out.append("        }")
    out.append("        Some(crate::command_registry::StoreCtx::Legacy { storage }) => {")
    out.append("            let storage = *storage;")
    if legacy_arms:
        out.append("            match &args {")
        for a in legacy_arms:
            out.append(arm_text(*a))
        out.append("                #[allow(unreachable_patterns)]")
        out.append(f'                _ => unreachable!("`aida {cli_name}` finished before storage init"),')
        out.append("            }")
        out.append("            Ok(crate::command_registry::Flow::Continue)")
    else:
        out.append(f'            unreachable!("`aida {cli_name}` has no centralized-store form")')
    out.append("        }")
    out.append("    }")
    out.append("}")
    reg = [
        "// trace:STORY-1728 | ai:claude",
        f"pub(crate) static {static}: crate::command_registry::CommandReg = crate::command_registry::CommandReg {{",
        f'    name: "{cli_name}",',
        f'    help_after: "{help_after}",',
        f"    command: <{args_ty} as clap::CommandFactory>::command,",
        "    policy: |_| crate::command_registry::CmdPolicy {",
        f"        stakeholder: crate::stakeholder_role::StakeholderAction::{stakeholder},",
        f"        per_write_auto_push: {'true' if push else 'false'},",
        f"        skip_role_restore_prompt: {'true' if skip else 'false'},",
        "    },",
        f"    run: crate::command_registry::Handler::Store({fn_name}),",
        "};",
    ]
    texts[mod_file] = texts[mod_file].rstrip("\n") + "\n\n" + "\n".join(struct_lines) + "\n\n" + "\n".join(out) + "\n\n" + "\n".join(reg) + "\n"

    rt = texts["command_registry.rs"].split("\n")
    at = rt.index("pub(crate) static REGISTRY: &[&CommandReg] = &[")
    end = rt.index("];", at)
    by_name = {}
    for e in rt[at + 1 : end]:
        m = re.search(r"&crate::(\w+)::(\w+),", e)
        nm = next(r[0] for r in regs if r[2] == m.group(1) and r[3] == m.group(2))
        by_name[nm] = e
    by_name[cli_name] = f"    &crate::{module}::{static},"
    rt[at + 1 : end] = [by_name[k] for k in sorted(by_name)]
    texts["command_registry.rs"] = "\n".join(rt)

    tp = os.path.join(SRC, "tests", "story_1728_command_registry_tests.rs")
    tt = open(tp).read()
    tt = re.sub(r"const LEGACY_TOP_LEVEL_COMMANDS: usize = (\d+);", lambda m: f"const LEGACY_TOP_LEVEL_COMMANDS: usize = {int(m.group(1)) - 1};", tt)
    if not args.dry_run:
        for n, t in texts.items():
            mc.write(n, t, False)
        open(tp, "w").write(tt)
        import subprocess
        subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "fix_scope.py"), mod_file], check=True, stdout=subprocess.DEVNULL)
    print(f"{v}: `aida {cli_name}` -> {module} store-phase (after `{help_after}`): early={len(early)} git={len(git_arms)} legacy={len(legacy_arms)} stakeholder={stakeholder} push={push} skip={skip}")
    for p in problems:
        print("  NOTE", p)


if __name__ == "__main__":
    main()
