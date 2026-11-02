#!/usr/bin/env python3
"""Resolve and verify in-page links in the generated Glossary chapter.

The book chapter (docs/cli/12-glossary.md) prints three discipline templates
on one page: machinery glossary, lifecycle vocabulary, product glossary. Each
template writes its links against itself, so two kinds of link break once
they share a page:

- Links between the templates (`machinery-glossary.md`,
  `lifecycle-vocabulary.md#frag`, `product-glossary.md`) point at pages the
  book does not publish. They become the in-page anchor of that section, or of
  the named heading inside it.
- A bare `#anchor` means a heading in its own template. When two sections
  share a heading (product "Role", machinery "role"), mdBook gives the later
  one a `-1` id, so the link has to name the heading in its own section.

Heading ids follow mdBook: lowercase, alphanumerics plus `-` and `_` kept,
whitespace to `-`, other characters dropped, and a repeated id gets `-<n>`.
Fenced blocks and inline code spans are left alone.

Usage:
  glossary-anchors.py PAGE            rewrite PAGE in place
  glossary-anchors.py --verify PAGE   check PAGE, change nothing

Both modes exit 1 and name the line when an in-page link has no heading, or
lands on a heading in another section while its own section has one with the
same name.
"""
# trace:STORY-2219 | ai:claude

import re
import sys

SECTIONS = {
    "Machinery glossary": "machinery-glossary.md",
    "Lifecycle vocabulary": "lifecycle-vocabulary.md",
    "Product glossary": "product-glossary.md",
}
HEADING = re.compile(r"^(#{1,6}) +(.*?) *#*$")
LINK = re.compile(r"\]\(([^)\s]*)\)")
CODE_SPAN = re.compile(r"(`[^`]*`)")


def base_id(text):
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]*>", "", text).replace("`", "").replace("*", "")
    out = []
    for ch in text.strip():
        if ch.isalnum() or ch in "-_":
            out.append(ch.lower())
        elif ch.isspace():
            out.append("-")
    return "".join(out)


def scan(lines):
    """Return (ids, uid_info, section_id, line_section, fenced_lines).

    ids:          (section, base id) -> first mdBook id with that base there
    uid_info:     mdBook id -> (section, base id)
    section_id:   template file name -> id of its H2
    line_section: section of each line (None before the first section)
    """
    counts, ids, uid_info, section_id = {}, {}, {}, {}
    line_section, fenced_lines = [], set()
    section, fenced = None, False
    for i, line in enumerate(lines):
        if line.startswith("```"):
            fenced = not fenced
            fenced_lines.add(i)
        elif fenced:
            fenced_lines.add(i)
        m = None if i in fenced_lines else HEADING.match(line)
        if m:
            base = base_id(m.group(2))
            n = counts.get(base, 0)
            counts[base] = n + 1
            uid = base if n == 0 else "%s-%d" % (base, n)
            if len(m.group(1)) == 2 and m.group(2) in SECTIONS:
                section = SECTIONS[m.group(2)]
                section_id[section] = uid
            ids.setdefault((section, base), uid)
            uid_info[uid] = (section, base)
        line_section.append(section)
    return ids, uid_info, section_id, line_section, fenced_lines


def links(line):
    """Yield (part index, match) for each link outside inline code spans."""
    parts = CODE_SPAN.split(line)
    return parts, [(j, m) for j in range(0, len(parts), 2) for m in LINK.finditer(parts[j])]


def rewrite(lines):
    ids, _, section_id, line_section, fenced_lines = scan(lines)
    errors, out = [], []
    for i, line in enumerate(lines):
        here = line_section[i]
        if here is None or i in fenced_lines:
            out.append(line)
            continue

        def resolve(m):
            page, _, frag = m.group(1).partition("#")
            if page and page not in section_id:
                return m.group(0)  # a real page outside this chapter
            sect = page or here
            if not frag:
                return "](#%s)" % section_id[sect]
            uid = ids.get((sect, frag))
            if uid is None:
                errors.append("line %d: no heading for #%s in %s" % (i + 1, frag, sect))
                return m.group(0)
            return "](#%s)" % uid

        parts = CODE_SPAN.split(line)
        for j in range(0, len(parts), 2):
            parts[j] = LINK.sub(resolve, parts[j])
        out.append("".join(parts))
    return out, errors


def verify(lines):
    ids, uid_info, section_id, line_section, fenced_lines = scan(lines)
    errors = []
    for i, line in enumerate(lines):
        here = line_section[i]
        if here is None or i in fenced_lines:
            continue
        _, found = links(line)
        for _, m in found:
            target = m.group(1)
            page = target.partition("#")[0]
            if page in SECTIONS.values():
                errors.append("line %d: link to unpublished page %s" % (i + 1, target))
                continue
            if not target.startswith("#"):
                continue
            uid = target[1:]
            if uid not in uid_info:
                errors.append("line %d: no heading for %s" % (i + 1, target))
                continue
            sect, base = uid_info[uid]
            own = ids.get((here, base))
            if own is not None and own != uid and uid not in section_id.values():
                errors.append(
                    "line %d: %s lands in %s, but %s has its own #%s"
                    % (i + 1, target, sect, here, own)
                )
    return errors


def main(argv):
    check = argv[:1] == ["--verify"]
    args = argv[1:] if check else argv
    if len(args) != 1:
        sys.stderr.write("usage: glossary-anchors.py [--verify] PAGE\n")
        return 2
    path = args[0]
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    if check:
        errors = verify(lines)
    else:
        lines, errors = rewrite(lines)
        errors = errors or verify(lines)
    if errors:
        sys.stderr.write("error: glossary anchors in %s:\n  %s\n" % (path, "\n  ".join(errors)))
        return 1
    if not check:
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
