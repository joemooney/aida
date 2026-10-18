#!/usr/bin/env python3
"""Move one top-level CLI command from the legacy `Command` enum into the
per-module command registry (STORY-1728).

    python3 scripts/story-1728/migrate_command.py <Variant> [--module <mod>] [--dry-run]

Handles the common pre-store shape: a single
`if let Command::<Variant> ... = &cli.command { ... }` handler in `run()` that
always returns. It

  1. cuts the variant (docs, attributes, fields) out of `cli.rs` and writes it
     into the owning module as a `#[derive(clap::Parser)]` struct;
  2. moves the handler body into that module as the registration's `run` fn;
  3. reads the command's policy from the stakeholder, role-restore and
     auto-push tables and removes its whole-variant entries from them;
  4. deletes the command's `unreachable!` arms from the storage dispatchers;
  5. adds the `REGISTRY` line (sorted) and lowers the legacy ratchet.

Anything it cannot do mechanically (a sub-pattern policy entry, a handler that
reads `run()` locals, test references) it reports instead of guessing; the
compiler and the golden help snapshot catch the rest.
trace:STORY-1728 | ai:claude
"""
import argparse
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SRC = os.path.join(ROOT, "aida-cli-lib", "src")


def read(name):
    with open(os.path.join(SRC, name)) as f:
        return f.read()


def write(name, text, dry):
    if dry:
        return
    with open(os.path.join(SRC, name), "w") as f:
        f.write(text)


def kebab(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "-", name).lower()


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def brace_end(lines, i):
    """Index of the line closing the brace opened at or after line i."""
    depth = 0
    seen = False
    for j in range(i, len(lines)):
        code = re.sub(r'"(?:[^"\\]|\\.)*"', '""', lines[j].split("//")[0])
        depth += code.count("{") - code.count("}")
        depth += code.count("(") * 0
        if "{" in code:
            seen = True
        if seen and depth <= 0:
            return j
    raise SystemExit("unbalanced braces")


def leading_comments(lines, i, include_attrs=True):
    """First index of the doc/comment/attribute run directly above line i,
    including attributes that span several lines (`#[clap(group(` … `))]`)."""
    j = i
    while j > 0:
        s = lines[j - 1].strip()
        if s.startswith("//") or (include_attrs and s.startswith("#[")):
            j -= 1
            continue
        if include_attrs and s.endswith("]"):
            k = j - 1
            while k > 0 and not lines[k].strip().startswith("#["):
                k -= 1
            if lines[k].strip().startswith("#["):
                j = k
                continue
        break
    return j


# ---------------------------------------------------------------- cli.rs


def cut_variant(cli, variant):
    lines = cli.split("\n")
    enum_at = next(i for i, l in enumerate(lines) if l == "pub enum Command {")
    enum_end = next(i for i in range(enum_at, len(lines)) if lines[i] == "}")
    pat = re.compile(r"^    " + variant + r"(\b|$)")
    at = next(
        (i for i in range(enum_at, enum_end) if pat.match(lines[i])), None
    )
    if at is None:
        raise SystemExit(f"variant {variant} not found in Command")
    head = lines[at].strip()
    if head.endswith("{"):
        end = brace_end(lines, at)
        kind = "struct"
    elif head.endswith(","):
        end = at
        kind = "tuple" if "(" in head else "unit"
    else:
        raise SystemExit(f"unrecognized variant header: {head}")
    start = leading_comments(lines, at)
    block = lines[start : end + 1]
    # drop one trailing blank line so the enum keeps single spacing
    stop = end + 1
    if stop < len(lines) and lines[stop].strip() == "" and start > 0 and lines[start - 1].strip() == "":
        stop += 1
    rest = lines[:start] + lines[stop:]
    return block, at - start, kind, "\n".join(rest)


def split_attr(inner):
    """Split a clap attribute list on top-level commas."""
    parts, depth, cur, quoted = [], 0, "", False
    for ch in inner:
        if ch == '"' and not cur.endswith("\\"):
            quoted = not quoted
        if not quoted:
            if ch in "([":
                depth += 1
            elif ch in ")]":
                depth -= 1
            elif ch == "," and depth == 0:
                parts.append(cur)
                cur = ""
                continue
        cur += ch
    if cur.strip():
        parts.append(cur)
    return parts


def subcommand_kind(cli, ty):
    m = re.search(r"#\[derive\(([^)]*)\)\]\s*(?:#\[[^\]]*\]\s*)*pub (enum|struct) " + ty + r"\b", cli)
    if not m:
        return None
    derives = m.group(1)
    if "Subcommand" in derives:
        return "subcommand"
    if "Args" in derives or "Parser" in derives:
        return "flatten"
    return None


def parser_struct(block, head_off, kind, variant, cli):
    """Render the variant as a `#[derive(clap::Parser)]` struct."""
    pre = [l[4:] if l.startswith("    ") else l for l in block[:head_off]]
    head = block[head_off].strip()
    # Group each attribute with its continuation lines.
    attrs, docs, cur = [], [], None
    for l in pre:
        if cur is not None:
            cur += "\n" + l
            if cur.count("[") <= cur.count("]"):
                attrs.append(cur)
                cur = None
        elif l.startswith("#["):
            if l.count("[") > l.count("]"):
                cur = l
            else:
                attrs.append(l)
        else:
            docs.append(l)
    name = None
    variant_mode = None
    keep_attrs = []
    for a in attrs:
        m = re.match(r"#\[(clap|command)\((.*)\)\]$", a)
        if not m:
            keep_attrs.append(a)
            continue
        inner = m.group(2)
        nm = re.search(r'\bname\s*=\s*"([^"]+)"', inner)
        if nm:
            name = nm.group(1)
        # `subcommand` / `flatten` are variant-only; the struct field carries them.
        parts = [p.strip() for p in split_attr(inner)]
        if any(p in ("subcommand", "flatten") for p in parts):
            variant_mode = next(p for p in parts if p in ("subcommand", "flatten"))
        parts = [p for p in parts if p not in ("subcommand", "flatten")]
        if parts:
            keep_attrs.append(f"#[{m.group(1)}({', '.join(parts)})]")
    if name is None:
        name = kebab(variant)
        keep_attrs.insert(0, f'#[clap(name = "{name}")]')
    out = docs + ["#[derive(clap::Parser, Debug)]"] + keep_attrs
    binding = None
    if kind == "unit":
        out.append(f"pub(crate) struct {variant}Args {{}}")
    elif kind == "struct":
        out.append(f"pub(crate) struct {variant}Args {{")
        for l in block[head_off + 1 : -1]:
            body = l[4:] if l.startswith("    ") else l
            out.append(re.sub(r"^(    )(r#)?(\w+):", r"\1pub \2\3:", body))
        out.append("}")
    else:
        ty = re.match(variant + r"\((.+)\),$", head).group(1)
        sub = variant_mode or subcommand_kind(cli, ty.split("::")[-1])
        if variant_mode is None and sub == "subcommand":
            raise SystemExit(f"{variant}: newtype of a Subcommand without #[clap(subcommand)]; check by hand")
        if variant_mode is None:
            sub = "flatten"
        if sub is None:
            raise SystemExit(f"cannot tell whether {ty} is a Subcommand or Args")
        out.append(f"pub(crate) struct {variant}Args {{")
        out.append(f"    #[clap({sub})]")
        out.append(f"    pub inner: {ty},")
        out.append("}")
        binding = "inner"
    return name, out, binding


# ---------------------------------------------------------------- handler


def cut_handler(lib, variant):
    lines = lib.split("\n")
    run_at = next(i for i, l in enumerate(lines) if l.startswith("pub(crate) fn run() -> Result<()> {"))
    pat = re.compile(r"^    if let Command::" + variant + r"\b")
    hits = [i for i in range(run_at, len(lines)) if pat.match(lines[i])]
    if len(hits) != 1:
        raise SystemExit(f"expected one early handler for {variant}, found {len(hits)}")
    at = hits[0]
    # pattern runs until the line ending with `= &cli.command {` or `= &cli.command` + `{`
    j = at
    while "= &cli.command" not in lines[j]:
        j += 1
    pattern = " ".join(l.strip() for l in lines[at : j + 1])
    body_open = j if lines[j].rstrip().endswith("{") else j + 1
    end = brace_end(lines, body_open)
    start = leading_comments(lines, at, include_attrs=False)
    comments = [l[4:] if l.startswith("    ") else l for l in lines[start:at]]
    body = [l[8:] if l.startswith("        ") else l.lstrip() for l in lines[body_open + 1 : end]]
    stop = end + 1
    if stop < len(lines) and lines[stop].strip() == "" and lines[start - 1].strip() == "":
        stop += 1
    rest = lines[:start] + lines[stop:]
    m = re.match(r"if let Command::" + variant + r"\s*(.*?)\s*= &cli\.command\s*\{?$", pattern)
    pat_rest = m.group(1).strip()
    if pat_rest.startswith("(") and re.match(r"\((\w+)Command::", pat_rest):
        raise SystemExit(f"{variant}: handler matches only one subcommand: {pat_rest}")
    return comments, pat_rest, body, "\n".join(rest)


def destructure(pat_rest, args_ty, binding):
    if not pat_rest:
        return []
    if pat_rest.startswith("{"):
        return [f"let {args_ty} {pat_rest} = &args;"]
    m = re.match(r"\((\w+)\)$", pat_rest)
    if m and binding:
        return [f"let {m.group(1)} = &args.{binding};"]
    raise SystemExit(f"{args_ty}: unsupported pattern {pat_rest}")


# ---------------------------------------------------------------- policy

WHOLE = r"Command::{v}(?:\s*\{{\s*\.\.\s*\}}|\(_\)|(?=\s*[|)=,\n]))"


def policy_for(variant, texts):
    """(stakeholder, auto_push, skip_role_restore, problems)."""
    problems = []
    st = texts["stakeholder_role.rs"]
    fn_at = st.index("pub(crate) fn stakeholder_cli_action(")
    fn_end = st.index("\n}\n", fn_at)
    fn = st[fn_at:fn_end]
    read_end = fn.index("StakeholderAction::Read")
    read_part, match_part = fn[:read_end], fn[read_end:]
    whole = re.compile(WHOLE.format(v=variant))
    any_ref = re.compile(r"Command::" + variant + r"\b")
    stakeholder = "Write"
    if whole.search(read_part):
        stakeholder = "Read"
    elif any_ref.search(read_part):
        problems.append("stakeholder Read list matches a sub-pattern")
    m = re.search(r"Command::" + variant + r"\b[^=]*=>\s*StakeholderAction::(\w+)", match_part)
    if m:
        stakeholder = m.group(1)
    elif re.search(r"Command::" + variant + r"\b", match_part):
        problems.append("stakeholder match arm is shared or nested")

    role = texts["role.rs"]
    r_at = role.index("pub(crate) fn role_restore_prompt_should_skip(")
    r_fn = role[r_at : role.index("\n}\n", r_at)]
    skip = bool(whole.search(r_fn))

    lib = texts["lib_part1.rs"]
    a_at = lib.index("pub(crate) fn command_triggers_per_write_auto_push(")
    a_fn = lib[a_at : lib.index("\n}\n", a_at)]
    push = False
    if any_ref.search(a_fn):
        if re.search(WHOLE.format(v=variant) + r"[^=]*=>\s*true", a_fn) or re.search(
            r"\|\s*" + WHOLE.format(v=variant), a_fn
        ):
            push = True
        else:
            problems.append("auto-push table matches a sub-pattern")
    return stakeholder, push, skip, problems


def strip_from_tables(variant, texts):
    """Remove whole-variant alternatives from the three policy tables."""
    alt = WHOLE.format(v=variant)
    for name, fn_name in [
        ("stakeholder_role.rs", "pub(crate) fn stakeholder_cli_action("),
        ("role.rs", "pub(crate) fn role_restore_prompt_should_skip("),
        ("lib_part1.rs", "pub(crate) fn command_triggers_per_write_auto_push("),
    ]:
        t = texts[name]
        at = t.index(fn_name)
        end = t.index("\n}\n", at)
        fn = t[at:end]
        # `| Command::V { .. }` continuation line
        fn = re.sub(r"\n\s*\|\s*" + alt + r"(?=\s*\n)", "", fn)
        # leading alternative `Command::V { .. }\n | ...` -> drop and promote next
        fn = re.sub(r"(\n\s*)" + alt + r"\n\s*\|\s*", r"\1", fn)
        # single-line match arm `Command::V(..) => X,`
        fn = re.sub(r"\n\s*" + alt + r"\s*=>\s*[^\n]*,(?=\n)", "", fn)
        texts[name] = t[:at] + fn + t[end:]


# ---------------------------------------------------------------- arms


def strip_unreachable_arms(text, variant):
    alt = WHOLE.format(v=variant)
    one = re.compile(r"(?:\n[ \t]*//[^\n]*)*\n[ \t]*" + alt + r"\s*=>\s*unreachable!\([^\n]*\),[ \t]*(?=\n)")
    three = re.compile(
        r"(?:\n[ \t]*//[^\n]*)*\n[ \t]*" + alt
        + r"\s*=>\s*\{(?:\n[ \t]*//[^\n]*)*\n[ \t]*unreachable!\([^\n]*\);?\n[ \t]*\}(?=\n)"
    )
    text, a = one.subn("", text)
    text, b = three.subn("", text)
    return text, a + b


# ---------------------------------------------------------------- registry


def legacy_names(cli):
    lines = cli.split("\n")
    at = next(i for i, l in enumerate(lines) if l == "pub enum Command {")
    end = next(i for i in range(at, len(lines)) if lines[i] == "}")
    names = []
    for i in range(at + 1, end):
        m = re.match(r"^    ([A-Z]\w*)(\s*\{|\(|,|$)", lines[i])
        if not m or m.group(1) == "Registered":
            continue
        k = leading_comments(lines, i)
        name = kebab(m.group(1))
        for a in lines[k:i]:
            n = re.search(r'#\[(?:clap|command)\(.*\bname\s*=\s*"([^"]+)"', a)
            if n:
                name = n.group(1)
        names.append(name)
    return names


def registered(src_dir):
    regs = []
    for f in sorted(os.listdir(src_dir)):
        if not f.endswith(".rs"):
            continue
        t = open(os.path.join(src_dir, f)).read()
        for m in re.finditer(
            r'pub\(crate\) static (\w+):\s*(?:crate::command_registry::)?CommandReg\s*=\s*(?:crate::command_registry::)?CommandReg\s*\{\s*name:\s*"([^"]+)",\s*help_after:\s*"([^"]+)"',
            t,
        ):
            regs.append((m.group(2), m.group(3), f[:-3], m.group(1)))
    return regs


def built_order(legacy, regs):
    """Mirror `command_registry::built_command` placement."""
    order = list(legacy)
    pending = sorted(regs, key=lambda r: r[0], reverse=True)
    while pending:
        before = len(pending)
        keep = []
        for r in pending:
            if r[1] in order:
                order.insert(order.index(r[1]) + 1, r[0])
            else:
                keep.append(r)
        pending = keep
        if len(pending) == before:
            order.extend(r[0] for r in pending)
            break
    return order


def infer_module(body):
    """The module owning the handler: a module-qualified call first, then the
    defining module of the first `handle_*`/`run_*`/`print_*` call, then of any
    call. Crate-root `lib_part*.rs` includes and `kernel` helpers never own."""
    files = {f[:-3] for f in os.listdir(SRC) if f.endswith(".rs")}
    calls = [(m.group(1), m.group(2)) for m in re.finditer(r"\b(?:crate::)?(?:(\w+)::)?(\w+)\(", body)]
    for qual, _ in calls:
        if qual in files and not qual.startswith("lib") and qual not in ("kernel", "cli"):
            return qual

    def defining(fn):
        for f in sorted(files):
            if f.startswith("lib") or f in ("cli", "kernel"):
                continue
            text = open(os.path.join(SRC, f + ".rs")).read()
            if re.search(r"^pub\(crate\) fn " + fn + r"\b", text, re.M):
                return f
        return None

    for want in (lambda fn: re.match(r"(handle|run|print)_", fn), lambda fn: True):
        for qual, fn in calls:
            if qual is None and want(fn) and fn not in ("Ok", "Err", "Some", "format", "matches"):
                f = defining(fn)
                if f:
                    return f
    return None


# ---------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("variant")
    ap.add_argument("--module")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    v = args.variant

    names = ["cli.rs", "lib_part1.rs", "git_backend_cmd.rs", "stakeholder_role.rs", "role.rs", "command_registry.rs"]
    texts = {n: read(n) for n in names}

    legacy_before = legacy_names(texts["cli.rs"])
    block, head_off, kind, texts["cli.rs"] = cut_variant(texts["cli.rs"], v)
    cli_name, struct_lines, binding = parser_struct(block, head_off, kind, v, texts["cli.rs"])

    comments, pat_rest, body, texts["lib_part1.rs"] = cut_handler(texts["lib_part1.rs"], v)

    module = args.module or infer_module("\n".join(body))
    if module is None:
        raise SystemExit(f"{v}: cannot infer the owning module; pass --module")
    mod_file = module + ".rs"
    if mod_file not in texts:
        texts[mod_file] = read(mod_file)

    stakeholder, push, skip, problems = policy_for(v, texts)
    strip_from_tables(v, texts)

    removed = 0
    for n in ("lib_part1.rs", "git_backend_cmd.rs"):
        texts[n], c = strip_unreachable_arms(texts[n], v)
        removed += c

    legacy_after = [n for n in legacy_before if n != cli_name]
    regs = registered(SRC)
    order = built_order(legacy_before, regs)
    idx = order.index(cli_name)
    help_after = order[idx - 1] if idx > 0 else None
    if help_after is None:
        raise SystemExit(f"{v}: first command has no predecessor to anchor to")

    if re.search(r"\b(?:struct|enum|type) " + v + r"Args\b", texts[mod_file]):
        struct_lines = [l.replace(f"{v}Args", f"{v}CmdArgs") for l in struct_lines]
        args_ty = f"{v}CmdArgs"
    else:
        args_ty = f"{v}Args"
    static = snake(v).upper()
    fn_name = f"run_{snake(v)}"
    uses_ctx = any("cli.globals" in l for l in body)
    body = [l.replace("cli.globals", "ctx.globals") for l in body]
    # Self-module calls become `self::` so the fn-local `use crate::*;` the
    # scope fix adds can never resolve them to a crate-root namesake.
    body = [l.replace(f"crate::{module}::", "self::") for l in body]
    body = [re.sub(r"(?<![\w:])" + module + r"::(?=\w+\()", "self::", l) for l in body]
    leftovers = [l.strip() for l in body if re.search(r"\b(cli|raw_args|storage|backend|project_root_hint)\b", l) and "ctx.globals" not in l]
    run_lines = []
    run_lines.append(f"fn {fn_name}(matches: &clap::ArgMatches, {'ctx' if uses_ctx else '_ctx'}: &crate::command_registry::DispatchCtx) -> anyhow::Result<()> {{")
    if kind != "unit":
        run_lines.append(f"    let args = <{args_ty} as clap::FromArgMatches>::from_arg_matches(matches)?;")
    for d in destructure(pat_rest, args_ty, binding):
        run_lines.append("    " + d)
    run_lines += ["    " + l if l.strip() else "" for l in body]
    run_lines.append("}")
    if kind == "unit":
        run_lines[0] = run_lines[0].replace("matches:", "_matches:")

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
        f"    run: crate::command_registry::Handler::Early({fn_name}),",
        "};",
    ]
    generated = ["", *struct_lines, "", *comments, *run_lines, "", *reg, ""]
    texts[mod_file] = texts[mod_file].rstrip("\n") + "\n" + "\n".join(generated)
    if "use anyhow::Result;" not in texts[mod_file] and "use anyhow::{" not in texts[mod_file] and "use crate::*;" not in texts[mod_file]:
        problems.append(f"{mod_file}: add `use anyhow::Result;`")

    # REGISTRY line, sorted by name
    rt = texts["command_registry.rs"].split("\n")
    at = rt.index("pub(crate) static REGISTRY: &[&CommandReg] = &[")
    end = rt.index("];", at)
    entries = rt[at + 1 : end]
    by_name = {}
    for e in entries:
        m = re.search(r"&crate::(\w+)::(\w+),", e)
        nm = next(r[0] for r in regs if r[2] == m.group(1) and r[3] == m.group(2))
        by_name[nm] = e
    by_name[cli_name] = f"    &crate::{module}::{static},"
    rt[at + 1 : end] = [by_name[k] for k in sorted(by_name)]
    texts["command_registry.rs"] = "\n".join(rt)

    # ratchet
    tp = os.path.join(SRC, "tests", "story_1728_command_registry_tests.rs")
    tt = open(tp).read()
    tt = re.sub(
        r"const LEGACY_TOP_LEVEL_COMMANDS: usize = (\d+);",
        lambda m: f"const LEGACY_TOP_LEVEL_COMMANDS: usize = {int(m.group(1)) - 1};",
        tt,
    )

    for n, t in texts.items():
        write(n, t, args.dry_run)
    if not args.dry_run:
        with open(tp, "w") as f:
            f.write(tt)

    if not args.dry_run:
        import subprocess
        subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "fix_scope.py"), mod_file], check=True, stdout=subprocess.DEVNULL)
    print(f"{v}: `aida {cli_name}` -> {module} (after `{help_after}`), stakeholder={stakeholder} push={push} skip_restore={skip}, removed {removed} unreachable arm(s)")
    for p in problems:
        print(f"  NOTE {p}")
    for l in leftovers:
        print(f"  CHECK body uses run() local: {l}")


if __name__ == "__main__":
    main()
