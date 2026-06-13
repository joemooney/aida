#!/usr/bin/env python3
"""Doc-intent gate: surface-changing diffs must mark doc-impact at a spec.

# trace:STORY-603

The substrate-as-bouncer half of the doc-intent protocol (STORY-603 part 1).
A confident agent will skip populating `interface_changes` unless a programmatic
gate refuses the change — so this is a CHECK, not a CLAUDE.md rule.

RULE
----
If a diff touches the CLI command/flag SURFACE (`aida-cli/src/cli.rs`) OR the
agent-facing skill/command SET (`.claude/skills/`, `.claude/commands/`, or their
`aida-core/templates/{skills,commands}/` masters), then at least one spec the diff
REFERENCES must carry doc-impact intent — i.e. non-empty `interface_changes`
(STORY-542) OR a `docs:impacted` tag. Otherwise the change is a "blind surface
edit": the surface moved but no spec captured the WHY for the documenter.

WHERE SPEC IDs COME FROM
------------------------
The diff's commit messages (any `(SPEC-ID)` trailer or inline `SPEC-ID`) and any
`trace:SPEC-ID` / `# trace:SPEC-ID` comments in the added lines. This mirrors how
`aida pull` discovers which specs a merge references.

WHERE SPEC METADATA COMES FROM
------------------------------
The orphan `aida-store` branch (`git show aida-store:objects/.../<ID>.yaml`),
falling back to the local `.aida-store/` worktree if attached. This works in CI
(where the worktree is gitignored/absent) as long as the `aida-store` ref is
fetched — no `aida` binary or attached worktree required.

USAGE
-----
    python3 docs/cli/verify-interface-changes.py            # HEAD vs origin/main
    python3 docs/cli/verify-interface-changes.py <base>     # HEAD vs <base>
    python3 docs/cli/verify-interface-changes.py <base> <head>

Exit 0 = clean (no surface change, or surface change with a marked spec).
Exit 1 = surface change with no doc-impact-marked spec (hard fail).
Exit 2 = could not resolve the store (can't verify) — soft, prints SKIP.
"""
import os
import re
import subprocess
import sys

REPO = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True
).stdout.strip()

# Surface globs: a touched path matching any of these arms the gate.
SURFACE_PATHS = (
    "aida-cli/src/cli.rs",  # CLI command/flag definitions
)
SURFACE_DIRS = (
    ".claude/skills/",
    ".claude/commands/",
    "aida-core/templates/skills/",
    "aida-core/templates/commands/",
)

SPEC_RE = re.compile(r"\b((?:STORY|TASK|BUG|EPIC|SPIKE|ADR|FR|SR|UR|CR|CON|VIS|PRIN|TERM|SPEC)-[0-9]+)\b")
STORE_BRANCH = "aida-store"


def sh(args):
    return subprocess.run(args, capture_output=True, text=True, cwd=REPO)


def resolve_range(argv):
    """Return (base, head) refs to diff."""
    if len(argv) >= 3:
        return argv[1], argv[2]
    if len(argv) == 2:
        return argv[1], "HEAD"
    # default: prefer origin/main, fall back to main, then the merge-base
    for base in ("origin/main", "main"):
        if sh(["git", "rev-parse", "--verify", base]).returncode == 0:
            mb = sh(["git", "merge-base", base, "HEAD"]).stdout.strip()
            return (mb or base), "HEAD"
    return "HEAD~1", "HEAD"


def changed_files(base, head):
    out = sh(["git", "diff", "--name-only", f"{base}...{head}"]).stdout
    return [l for l in out.splitlines() if l]


def touches_surface(files):
    hits = []
    for f in files:
        if f in SURFACE_PATHS or any(f.startswith(d) for d in SURFACE_DIRS):
            hits.append(f)
    return hits


def referenced_specs(base, head):
    """SPEC-IDs from commit messages + added trace comments in the range."""
    specs = set()
    msgs = sh(["git", "log", "--format=%B", f"{base}..{head}"]).stdout
    specs.update(SPEC_RE.findall(msgs))
    # added lines only (the '+' side of the diff)
    diff = sh(["git", "diff", f"{base}...{head}"]).stdout
    for line in diff.splitlines():
        if line.startswith("+") and "trace:" in line:
            specs.update(SPEC_RE.findall(line))
    return specs


def load_spec_yaml(spec_id):
    """Return the raw YAML for a spec, from aida-store or the local worktree."""
    # 1. orphan branch (works in CI without an attached worktree)
    ls = sh(["git", "ls-tree", "-r", STORE_BRANCH, "--name-only"])
    if ls.returncode == 0:
        for path in ls.stdout.splitlines():
            if path.endswith(f"/{spec_id}.yaml"):
                show = sh(["git", "show", f"{STORE_BRANCH}:{path}"])
                if show.returncode == 0:
                    return show.stdout
    # 2. local attached worktree
    for root, _dirs, fnames in os.walk(os.path.join(REPO, ".aida-store", "objects")):
        if f"{spec_id}.yaml" in fnames:
            with open(os.path.join(root, f"{spec_id}.yaml")) as fh:
                return fh.read()
    return None


def _block_body(yaml_text, header_re):
    """Return the lines belonging to a top-level `header:` block (its
    contiguous indented / list-item body), or [] if the header is absent.
    Stops at the next top-level (column-0, non-blank) key so prose elsewhere
    in the YAML can't leak into the match."""
    m = re.search(header_re, yaml_text, re.M)
    if not m:
        return []
    body = []
    for line in yaml_text[m.end():].splitlines():
        if line.strip() == "":
            continue
        if not line[0].isspace() and not line.startswith("-"):
            break  # dedented to the next top-level key
        body.append(line)
    return body


def spec_marks_doc_impact(yaml_text):
    """True if the spec has non-empty interface_changes OR a docs:impacted tag.

    Lightweight text scan (no pyyaml dependency in CI). Scoped to the actual
    `tags:` / `interface_changes:` blocks — NOT the whole body — so prose that
    merely MENTIONS `docs:impacted` or `interface_changes` (e.g. this very
    spec's description) does not count as a mark.
    """
    if yaml_text is None:
        return None  # unresolved
    # tags: block — a `- docs:impacted` list item
    for line in _block_body(yaml_text, r"^tags:\s*$"):
        if re.match(r"^\s*-\s+docs:impacted\s*$", line):
            return True
    # interface_changes: block — at least one `- ` list item under cli/mcp/tui/other
    if any(re.match(r"^\s+-\s+", line) for line in _block_body(yaml_text, r"^interface_changes:\s*$")):
        return True
    return False


def main():
    base, head = resolve_range(sys.argv)
    files = changed_files(base, head)
    surface_hits = touches_surface(files)

    if not surface_hits:
        print(f"OK doc-intent — diff {base}..{head} touches no CLI/skill surface; gate not armed")
        sys.exit(0)

    print("doc-intent gate ARMED — surface-changing files in this diff:")
    for f in surface_hits:
        print(f"    {f}")

    specs = referenced_specs(base, head)
    if not specs:
        print(
            "FAIL doc-intent — surface changed but the diff references no spec.\n"
            "    A CLI/skill surface change must trace to a spec. Add a (SPEC-ID)\n"
            "    commit trailer or a `// trace:SPEC-ID` comment, then mark doc-impact\n"
            "    on that spec (populate interface_changes or add a `docs:impacted` tag)."
        )
        sys.exit(1)

    print(f"referenced spec(s): {', '.join(sorted(specs))}")

    marked = []
    unresolved = []
    unmarked = []
    for sid in sorted(specs):
        verdict = spec_marks_doc_impact(load_spec_yaml(sid))
        if verdict is True:
            marked.append(sid)
        elif verdict is None:
            unresolved.append(sid)
        else:
            unmarked.append(sid)

    if marked:
        print(f"OK doc-intent — doc-impact marked on: {', '.join(marked)}")
        sys.exit(0)

    if unresolved and not unmarked:
        print(
            f"SKIP doc-intent — could not resolve spec(s) {', '.join(unresolved)} "
            f"from the '{STORE_BRANCH}' branch or a local worktree; cannot verify.\n"
            f"    (Fetch the store: git fetch origin {STORE_BRANCH}:{STORE_BRANCH})"
        )
        sys.exit(2)

    print(
        "FAIL doc-intent — surface changed but NO referenced spec marks doc-impact.\n"
        f"    Referenced: {', '.join(sorted(specs))}\n"
        "    Fix: on the shaping spec, populate `interface_changes` (value-framed cli/\n"
        "    mcp/tui deltas — see `aida queue done --interface-cli ...`) OR add a\n"
        "    `docs:impacted` tag (`aida edit <spec> --tags docs:impacted`). This is the\n"
        "    doc-intent protocol: the spec is the source of WHY the documenter needs."
    )
    sys.exit(1)


if __name__ == "__main__":
    main()
