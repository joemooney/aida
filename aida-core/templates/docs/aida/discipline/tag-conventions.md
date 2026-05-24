# Tag conventions

Tags are AIDA's lightweight, free-form way to describe a spec from angles
the type/feature/status fields don't capture — surface area touched,
provenance, pattern, severity. Left untouched the tag space drifts into a
flat soup where `auto-complete-orchestrator`, `orchestrator`, and
`queue-work` all describe the same surface but never group together. This
page is the convention that keeps the namespace queryable.

The rules below are an AIDA-wide convention, not just this project's. The
same vocabulary should appear in every AIDA-using repo so cross-project
searches and the shared substrate stay coherent.

## The four rules

### 1. Subcommand-identifying tags use the `aida:<subcommand>[:<verb>][:<sub-verb>]` colon-namespaced form

A tag that names *which CLI surface* a spec touches goes in the `aida:`
namespace, with colons separating the nested verbs.

| Surface | Tag |
|---|---|
| `aida status` | `aida:status` |
| `aida queue list` | `aida:queue:list` |
| `aida queue work` | `aida:queue:work` |
| `aida db sync --pull` | `aida:db:sync:pull` |
| `aida session end` | `aida:session:end` |
| `aida cache rebuild` | `aida:cache:rebuild` |
| `aida mcp-serve` | `aida:mcp-serve` (the binary name is `mcp-serve`, hyphenated) |

The payoff is prefix-glob filtering:

```bash
aida list --tags 'aida:queue:*'    # everything touching the queue subcommand
aida list --tags 'aida:db:*'       # everything touching db plumbing
```

Without the namespace, a search for "queue-related specs" has to enumerate
every flat tag (`queue-list`, `queue-work`, `queue-progress`, …); with it,
one glob is the whole set.

### 2. Behavior / pattern / provenance / severity tags stay flat

Tags that describe a *characteristic* of the spec — not a surface — stay
in the flat namespace.

| Category | Examples |
|---|---|
| Behavior / role | `orchestrator`, `advisor`, `implementer`, `reviewer` |
| Pattern | `ceiling-pattern`, `wedge-vs-pile`, `substrate-as-bouncer` |
| Provenance | `from-self-test`, `from-user-direction`, `from-review`, `from-postmortem` |
| Severity | `papercut`, `paper-cut` (one of these — pick one and stick to it) |
| Scope tag | `cli-ux`, `scaffolding`, `tui`, `mcp`, `tests` |

These don't have a hierarchy to flatten and they don't compose with
prefix-globs (no one wants "everything tagged `from-*`"). Adding `aida:`
in front would be noise.

### 3. Existing colon-namespaced conventions continue unchanged

Several namespaces predate this convention and keep their own prefix
because they aren't naming a CLI surface — they're naming a *relationship*
or a *bounded value*.

| Namespace | Meaning | Example |
|---|---|---|
| `batch:<name>` | Membership in a batch drain (TASK-229) | `batch:overnight-2026-05-22` |
| `lifecycle:<state>` | Sub-classification of lifecycle / complexity | `lifecycle:trivial` |
| `severity:<level>` | Severity grading | `severity:cosmetic` |
| `parent:<SPEC-ID>` | Explicit parent linkage (when typed relationship missing) | `parent:EPIC-31` |
| `depends-on:<phase>` | Phase ordering inside an epic | `depends-on:phase-1` |
| `subsumes:<SPEC-ID>` | Spec absorbs an older spec's scope | `subsumes:TASK-204` |
| `from-review:<PR-N>` | Spec was filed during PR review | `from-review:PR-193` |
| `kind:<classification>` | Free-form sub-typing | `kind:bug-spotted` |
| `sibling-of-<SPEC-ID>` | Symmetric sibling pointer | `sibling-of-TASK-511` |

These are stable. Don't rename them into `aida:*` — they're not CLI
surfaces, and the existing form is already queryable.

### 4. Multi-touch specs get multiple subcommand tags

A spec that changes both `aida status` rendering and `aida queue list`
output gets *both* tags:

```bash
aida edit TASK-501 --tags 'aida:status,aida:queue:list'
```

This is the whole point of the namespace — one spec, multiple surfaces,
all discoverable from a single `aida list --tags 'aida:queue:*'` query.

## Anti-patterns to avoid

- **Hyphenated CLI surface tags.** `queue-work`, `db-sync-pull`,
  `session-end` — these flatten the hierarchy and don't compose with
  prefix-glob. Use the colon form.
- **Synonyms.** `orchestrator` and `auto-complete-orchestrator` for the
  same concept fragment the tag space. Pick one and keep it.
- **Tags that duplicate `type`/`feature`/`status`.** A `bug` tag on a
  `Type: Bug` spec is noise. A `done` tag on a `Status: Done` spec is
  worse — it goes stale the moment the status changes.
- **One-shot tags.** A tag used on exactly one spec is a comment
  pretending to be metadata. Either it generalizes to a family (keep) or
  it's session noise (drop).

## When to add a new namespace

If a new dimension genuinely doesn't fit anywhere above — and you can name
three or more specs that would carry it — add a namespace and document it
here. The bar is intentionally high: tag-namespace drift is the failure
mode this page exists to prevent.

## Related

- [`machinery-glossary.md`](machinery-glossary.md) — definitions of the
  machinery terms (orchestrator, phase, drain, lease, …) that frequently
  appear as flat behavior tags.
- [`lifecycle-vocabulary.md`](lifecycle-vocabulary.md) — the lifecycle
  state vocabulary, which uses `lifecycle:<state>` for sub-classifications
  beyond the seven primary statuses.
