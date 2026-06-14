#!/usr/bin/env python3
"""Documenter: assemble an intent-bearing manual-entry draft from the SPEC GRAPH.

# trace:TASK-794
# trace:TASK-795

STORY-603 part 2 (+ part 3 backlink, TASK-795). The overnight manual was verified against `--help` (the WHAT)
but did NOT consume spec INTENT (the WHY) — a "blind redetermination," not
intent-bearing docs. This documenter fixes that by deriving each draft entry
from THREE sources, not one:

  1. `--help`            — WHAT the command does (the surface / fact source).
  2. `interface_changes` — the per-spec cli/mcp/tui DELTA a spec introduced
                           (STORY-542), resolved from the `aida-store` branch by
                           REUSING the part-1 resolver (no re-derivation).
  3. spec description    — the WHY (intent + the `## Acceptance` section the spec
                           carries inline), from the shaping spec(s).

This is the generator half of the doc-intent protocol. `verify-interface-changes.py`
(part 1) is the bouncer that forces specs to MARK doc-impact; this script is the
documenter that CONSUMES those marks into a draft a human (or `aida-tutor`) edits
into the chapter. It writes a DRAFT — it never edits a chapter in place — and it
emits NO SPEC-IDs in the user-facing body (the shaping-spec list goes in an
HTML-comment provenance block the verify-manual leak check ignores), so the
output drops straight into the manual's no-SPEC-ID convention.

HOW A COMMAND MAPS TO ITS SHAPING SPEC(S)
-----------------------------------------
A spec "shaped" `aida <cmd>` if any line in its `interface_changes:` block names
`aida <cmd>` (or, for mcp tools, the bare tool token). This is deterministic and
substrate-grounded — the same metadata the part-1 gate enforces — so no LLM
judgement is needed to find the WHY behind a surface.

USAGE
-----
    python3 docs/cli/generate-entry.py <command>          # e.g. `human`, `queue done`
    python3 docs/cli/generate-entry.py <command> --json   # machine-readable (aida-tutor)
    python3 docs/cli/generate-entry.py --list             # commands with a shaping spec

Exit 0 = a draft (possibly help-only) printed. Exit 2 = store unresolvable.
"""
import json
import os
import re
import subprocess
import sys

# Reuse the part-1 store-resolution + interface_changes parsing verbatim — do
# NOT re-derive how a spec is loaded or how its blocks are scoped.
# trace:TASK-794
_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)
import importlib.util  # noqa: E402

_spec = importlib.util.spec_from_file_location(
    "verify_interface_changes", os.path.join(_HERE, "verify-interface-changes.py")
)
_vic = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_vic)

load_spec_yaml = _vic.load_spec_yaml  # (spec_id) -> raw YAML | None
_block_body = _vic._block_body  # (yaml, header_re) -> [lines]
sh = _vic.sh
STORE_BRANCH = _vic.STORE_BRANCH


def help_text(command):
    """`aida <command> --help` — the WHAT / fact source. '' if unavailable."""
    # trace:TASK-794
    try:
        r = subprocess.run(
            ["aida", *command.split(), "--help"],
            capture_output=True,
            text=True,
            timeout=20,
        )
        return r.stdout if r.returncode == 0 else ""
    except Exception:
        return ""


def all_spec_ids():
    """Every spec_id present on the store branch (or local worktree)."""
    # trace:TASK-794
    ids = set()
    ls = sh(["git", "ls-tree", "-r", STORE_BRANCH, "--name-only"])
    if ls.returncode == 0:
        for path in ls.stdout.splitlines():
            m = re.search(r"/([A-Z]+-[0-9]+)\.yaml$", path)
            if m:
                ids.add(m.group(1))
    if not ids:
        store = os.path.join(_vic.REPO, ".aida-store", "objects")
        for root, _d, fnames in os.walk(store):
            for fn in fnames:
                m = re.match(r"([A-Z]+-[0-9]+)\.yaml$", fn)
                if m:
                    ids.add(m.group(1))
    return ids


def interface_change_lines(yaml_text):
    """The `- ` list items under `interface_changes:` (across cli/mcp/tui/...)."""
    # trace:TASK-794
    lines = []
    for raw in _block_body(yaml_text, r"^interface_changes:\s*$"):
        m = re.match(r"^\s+-\s+(.*)$", raw)
        if m:
            lines.append(m.group(1).strip())
    return lines


def _yaml_scalar(yaml_text, key):
    """Best-effort read of a top-level scalar or `|-` block field (title /
    description). No pyyaml dependency — matches the part-1 text-scan style."""
    # trace:TASK-794
    m = re.search(rf"^{key}:\s*(.*)$", yaml_text, re.M)
    if not m:
        return ""
    inline = m.group(1).strip()
    if inline and inline not in ("|", "|-", ">", ">-"):
        # quoted single-line scalar
        return inline.strip("'\"")
    # block scalar: gather the contiguous indented body
    body = []
    after = yaml_text[m.end():].splitlines()
    for line in after:
        if line.strip() == "":
            body.append("")
            continue
        if not line[0].isspace():
            break
        body.append(line[2:] if line.startswith("  ") else line.lstrip())
    return "\n".join(body).strip()


def command_mentioned(command, line):
    """Does an interface_changes line name `aida <command>`?  Matches `aida
    <command>` as a token boundary so `aida queue` doesn't match `aida queue-x`."""
    # trace:TASK-794
    needle = f"aida {command}"
    return re.search(re.escape(needle) + r"\b", line) is not None


def shaping_specs(command, spec_ids):
    """Specs whose interface_changes name `aida <command>` — the WHY behind the
    surface. Returns [{id, title, why, deltas}] ordered by spec id."""
    # trace:TASK-794
    out = []
    for sid in sorted(spec_ids):
        yaml_text = load_spec_yaml(sid)
        if yaml_text is None:
            continue
        deltas = [
            line
            for line in interface_change_lines(yaml_text)
            if command_mentioned(command, line)
        ]
        if not deltas:
            continue
        out.append(
            {
                "id": sid,
                "title": _yaml_scalar(yaml_text, "title"),
                "why": _yaml_scalar(yaml_text, "description"),
                "deltas": deltas,
            }
        )
    return out


def build_entry(command):
    """Assemble the intent-bearing draft for `aida <command>` from the graph."""
    # trace:TASK-794
    ids = all_spec_ids()
    if not ids:
        return None  # store unresolvable
    specs = shaping_specs(command, ids)
    return {
        "command": command,
        "help": help_text(command),
        "shaping_specs": specs,
    }


# SPEC-IDs are developer breadcrumbs — they must never reach the user-facing
# manual body (the verify-manual leak check hard-fails on them). The documenter
# draft's prose strips them to a neutral placeholder so a copy-paste into a
# chapter can't seed a leak; the real IDs survive only in the HTML-comment
# provenance and the --json payload.
# trace:TASK-794
_SPECID_RE = re.compile(r"\b(?:STORY|TASK|BUG|EPIC|SPIKE|ADR|FR|SR|UR|CR|CON|VIS|PRIN|TERM|SPEC)-[0-9]+\b")


def _strip_spec_ids(text):
    # trace:TASK-794
    return _SPECID_RE.sub("a prior spec", text)


def _why_summary(why):
    """First real prose paragraph of a description — the intent in brief. Skips
    paragraphs that are ONLY a markdown heading (e.g. a lone `## Problem`) so the
    summary lands on substance, not a section label."""
    # trace:TASK-794
    for para in re.split(r"\n\s*\n", why.strip()):
        p = para.strip()
        # a paragraph that is nothing but `#`-heading line(s) carries no prose
        non_heading = [ln for ln in p.splitlines() if not re.match(r"^#+\s", ln)]
        text = "\n".join(non_heading).strip()
        if text:
            return text
        # heading-only paragraph with trailing words on the heading line itself
        stripped = re.sub(r"^#+\s*", "", p)
        if stripped and stripped != p.lstrip("#").strip():
            return stripped
    return ""


def render_markdown(entry):
    """A draft chapter entry following the manual's fixed-field shape. Carries NO
    SPEC-ID in the user-facing body; provenance goes in an HTML comment that the
    verify-manual leak check ignores."""
    # trace:TASK-794
    cmd = entry["command"]
    specs = entry["shaping_specs"]
    lines = [f"### `aida {cmd}`", ""]

    # WHAT — seeded from --help's first line so the documenter starts from fact.
    help_first = ""
    for hl in entry["help"].splitlines():
        if hl.strip():
            help_first = hl.strip()
            break
    lines.append(f"**One line** — {help_first or 'TODO: one-line summary (see `--help`).'}")
    lines.append("")

    # WHY — derived from the shaping spec(s)' description/intent.
    if specs:
        why = _why_summary(specs[0]["why"])
        lines.append(
            "**Why it exists** — "
            + (_strip_spec_ids(why) if why else "TODO: intent (the shaping spec carries no description).")
        )
        lines.append("")
        lines.append("**Interface delta this introduced**")
        for s in specs:
            for d in s["deltas"]:
                lines.append(f"- {_strip_spec_ids(d)}")
        lines.append("")
    else:
        lines.append(
            "**Why it exists** — TODO: no spec's `interface_changes` names this "
            "command yet. Mark doc-impact on the shaping spec (populate "
            "`interface_changes` or tag `docs:impacted`) so the documenter can "
            "derive the WHY."
        )
        lines.append("")

    lines.append(
        f"> Draft — `--help` is the fact source (`aida {cmd} --help`); "
        "the documenter fills the *when/why/when-not* fields from here."
    )
    lines.append("")

    # Backlink to the shaping spec(s) — machine-readable, NON-LEAKING (TASK-795).
    # SPEC-IDs live ONLY inside these HTML-comment markers, never in the rendered
    # prose; the verify-manual leak check ALLOWS SPEC-IDs inside the recognized
    # `doc-intent` / `trace` marker forms (and still hard-fails on bare IDs in
    # visible text). A reader/agent holding the requirement graph is then ONE HOP
    # from intent: parse the `trace:` token, `aida show <id>` the shaping spec.
    # Two complementary forms — a human-narrative provenance line and a terse,
    # grep-friendly `trace:` token that matches AIDA's code-trace convention.
    # trace:TASK-795
    if specs:
        ids = ", ".join(s["id"] for s in specs)
        trace_ids = ",".join(s["id"] for s in specs)
        lines.append(f"<!-- doc-intent: shaped by {ids} -->")
        lines.append(f"<!-- trace:{trace_ids} -->")
    else:
        lines.append("<!-- doc-intent: no shaping spec found via interface_changes -->")

    return "\n".join(lines) + "\n"


def main():
    argv = sys.argv[1:]
    as_json = "--json" in argv
    argv = [a for a in argv if a != "--json"]

    if argv and argv[0] == "--list":
        ids = all_spec_ids()
        if not ids:
            print("SKIP — could not resolve the store; cannot list shaping specs.")
            sys.exit(2)
        # collect every command any interface_changes line names
        cmds = {}
        for sid in sorted(ids):
            y = load_spec_yaml(sid)
            if y is None:
                continue
            for line in interface_change_lines(y):
                for m in re.finditer(r"\baida ([a-z][a-z0-9-]*(?: [a-z][a-z0-9-]*)?)", line):
                    cmds.setdefault(m.group(1), set()).add(sid)
        for cmd in sorted(cmds):
            print(f"{cmd}\t({', '.join(sorted(cmds[cmd]))})")
        sys.exit(0)

    if not argv:
        print(__doc__.strip().splitlines()[0])
        print("usage: python3 docs/cli/generate-entry.py <command> [--json|--list]")
        sys.exit(1)

    command = " ".join(argv).strip().removeprefix("aida ").strip()
    entry = build_entry(command)
    if entry is None:
        print(
            f"SKIP — could not resolve the '{STORE_BRANCH}' store; cannot derive "
            f"intent for `aida {command}`.\n"
            f"    (Fetch the store: git fetch origin {STORE_BRANCH}:{STORE_BRANCH})"
        )
        sys.exit(2)

    if as_json:
        print(json.dumps(entry, indent=2))
    else:
        print(render_markdown(entry))
    sys.exit(0)


if __name__ == "__main__":
    main()
