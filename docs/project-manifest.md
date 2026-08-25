# The AIDA project manifest — `.aida/project.toml`

A small, checked-in TOML file describing **what a project is**: its
description, why it exists, whether it is still alive, how far along it is, and
who owns it.

These are the facts no scanner can derive. Everything else about a repository —
languages, last commit, branch, remotes, size — a tool can work out by looking.
Intent cannot be looked up.

> **Reading this file from a non-AIDA tool?** Everything you need is on this
> page. Parse it with any TOML library, treat every field as optional, and skip
> to [Reading the manifest](#reading-the-manifest).

---

## Why it is checked in

Two failure modes motivated the standard, both from metadata kept outside the
repository:

- **Machine-local metadata does not travel.** Clone the repo somewhere else and
  the notes are gone.
- **Absolute-path keys orphan.** Move or rename a directory and the entry
  pointing at it silently detaches.

A file inside the repository has neither problem. It moves with the project,
survives a clone, and is reviewed like anything else.

## Why AIDA owns the format

AIDA writes this file; **consumers only read it**.

If a consumer defined its own format, every project would have to adopt that
particular consumer in order to be well-described. `aida init` is a command
these repositories already run, so the standard arrives on adoption that is
already happening, and any reader — a catalogue, a TUI, a phone client, a
`curl` one-liner — reads the same file.

This mirrors the arrangement that already works for `~/.ports`: one tool writes
it, everyone else reads it.

---

## Location

```
<project root>/.aida/project.toml
```

`.aida/` is otherwise gitignored deny-by-default — everything under it is
per-clone runtime state. The manifest is an explicit tracked exception,
allow-listed in `.gitignore`:

```gitignore
.aida/*
!.aida/config.toml
!.aida/project.toml
```

`aida init` adds that allow-line, including to projects initialized before the
manifest existed. **If you create the file by hand in an older project, check
the allow-line is there** — without it git ignores the manifest and it never
reaches anyone.

---

## Schema

Version 1. Every field is optional, including `schema` itself.

```toml
schema = 1

[project]
name           = "aida-hub"
description    = "A catalogue of my projects — so I stop forgetting them."
why            = "I kept starting things and losing track of them."
liveness       = "alive"
stage          = "alpha"
owner          = "joe"
classification = "tool"
repository     = "https://github.com/joemooney/aida-hub.git"
homepage       = "https://example.com/aida-hub"
tags           = ["rust", "personal"]
```

| Field | Type | Meaning |
| --- | --- | --- |
| `schema` | integer | Format version. Absent means `1`. |
| `project.name` | string | Display name. Fall back to the directory name when absent. |
| `project.description` | string | One line: what this is. |
| `project.why` | string | **Why it exists.** The field no tool can derive, and the reason the file is worth keeping. |
| `project.liveness` | enum | Is anyone working on it. See below. |
| `project.stage` | enum | How far along, independent of activity. See below. |
| `project.owner` | string | Who to ask. Free text — a name, a handle, a team. |
| `project.classification` | string | An author-asserted category. Free text; see below. |
| `project.repository` | string | Canonical remote URL. |
| `project.homepage` | string | Where it lives for users, if anywhere. |
| `project.tags` | array of strings | Free-form labels. |

### `liveness`

| Value | Meaning |
| --- | --- |
| `alive` | Actively worked on, or intended to be. |
| `parked` | Deliberately paused. Coming back to it is plausible. |
| `abandoned` | Done with. Kept for reference, not for work. |

**Absent is not a value.** It means *not stated*, which is different from every
stated value and should not be rendered as one. Do not default it to `alive`.

**A value you do not recognise is not an error.** See
[Unrecognised values](#unrecognised-values).

### `stage`

`idea` · `prototype` · `alpha` · `beta` · `stable` · `maintenance` · `sunset`

Orthogonal to `liveness`: a `parked` project still has a maturity, and a
`stable` one can be `parked`.

### `classification`

Free text **on purpose**. AIDA does not own a taxonomy here — baking one
consumer's vocabulary into the standard would make every other consumer wrong.

The consequence is real and you should plan for it: **a consumer with a closed
set of categories will meet values outside it.** A catalogue whose own
classification is, say, `project | worktree | ephemeral | unknown` cannot map an
arbitrary string onto that set one-to-one, and it should not try to. Treat an
unfamiliar `classification` as *the author asserted something I don't model* —
fall back to whatever you would have inferred without it, and, if you show it,
show it as the author's words rather than as one of your own categories.

Resolving that mismatch is the reader's job. Closing the set here would only
move the problem: it would make every consumer whose vocabulary differs from
AIDA's wrong instead.

### Unrecognised values

Any field's value may be one a given reader does not know — because the author
typed something idiosyncratic, or because a **newer AIDA added it**.

**Readers must degrade, never reject.** Specifically:

- Do not fail to parse the file. Keep every field you did understand; a manifest
  is not all-or-nothing, and losing `why` because `stage` was unfamiliar is a bad
  trade.
- Do not silently discard the value. Preserve it, so a tool that rewrites the
  file does not delete the author's words, and so it can be shown or reported.
- Do not coerce it into your nearest category. That invents a claim the author
  did not make.

AIDA's own reader works this way: an unknown `liveness` or `stage` is kept
verbatim, the rest of the manifest is read normally, and `aida doctor` reports
it as `project-manifest-unrecognised-value` — including the possibility that
your `aida` is simply older than the file.

This is what lets the schema grow. A value added in a later version must not
break the readers that already exist.

---

## Reading the manifest

Rules for a well-behaved reader:

1. **Absence is normal.** Most repositories will not have one. That is not a
   defect, a warning, or a lower score — just less information.
2. **Every field is optional.** Handle any subset, including an empty file.
3. **Unknown fields must not break you.** A newer AIDA may add optional fields;
   ignore what you do not recognise, so the standard can grow.
4. **Unknown *values* must not break you either.** See
   [Unrecognised values](#unrecognised-values) — keep the rest of the file, and
   do not coerce an unfamiliar value into your own vocabulary.
5. **A malformed file must not break you either.** Report it and carry on
   without it.
6. **Distinguish "not stated" from a value.** Especially for `liveness`.

### Layering with your own metadata

If you also keep your own per-machine notes, the recommended precedence is:

```
repo manifest  →  shared truth, travels with the project
machine-local  →  personal override, never leaves this machine
```

Local overrides the manifest, and **show which one a displayed value came
from**. A private note silently masquerading as the project's own statement is
how a catalogue starts lying to its reader.

---

## Writing the manifest

`aida init` creates it, **pre-filled** from what is already knowable:

- `name` — the directory name
- `description` — the opening paragraph of the README, with badges, HTML
  blocks, code fences and ASCII art skipped
- `repository` — the `origin` remote

Pre-filling is deliberate. An empty form is exactly how a metadata standard
goes stale in a week; a file that is already partly true invites the one edit
that makes it useful.

### Your edits are safe

`aida init` **leaves an existing manifest alone** — including
`aida init --refresh`. There is no template to overlay and no merge to get
wrong. Use `aida init --force` if you deliberately want it regenerated.

Everything after the pre-filled fields is yours. The file is ordinary TOML;
edit it with an editor.

---

## `aida doctor`

```bash
aida doctor --category project-manifest
```

Aliases: `manifest`, `manifests`, `project-manifests`, `project-metadata`.

| Finding | When |
| --- | --- |
| `project-manifest-malformed` | The file exists but cannot be parsed. |
| `project-manifest-unfilled` | Scaffolded and never completed — nothing recorded that a scan could not already work out. |
| `project-manifest-unrecognised-value` | A `liveness` or `stage` value this build does not know. The file is still read normally; the value may simply come from a newer AIDA. |
| `project-manifest-repository-drift` | The recorded `repository` no longer matches `origin`. |

**No manifest produces no finding.** A project without one is not unhealthy.

Remote-URL comparison is form-insensitive: `https://host/o/n.git`,
`git@host:o/n.git` and `ssh://git@host/o/n` are the same repository, so drift
is only reported when the repository genuinely differs.

---

## Related

- `docs/aida/discipline/` — the scaffolded discipline pack, same
  create-if-absent contract.
- `aida init --help` — `--force` regenerates scaffolded files.

<!-- trace:STORY-781 | ai:claude -->
