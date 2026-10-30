# Product documentation completion gap pass

<!-- trace:TASK-HOME-128 | ai:codex -->

Completion delta for EPIC-HOME-002, based on merged main `79ed6f56bf8f6dc34aaeb43f1b8d232cdf5b82aa` (PR-3103). This is author evidence for the advisor; the epic remains incomplete until this follow-up merges. Current guidance and publication links are checked separately from dated evidence and contextual/template candidates.

| promise | where it is met | gap |
|---|---|---|
| Inventory separates current guidance from historical evidence | [Original report](../reports/2026-10-29-documentation-review.md), [refreshed snapshot](../reports/2026-10-30-completion-snapshot.json), [inventory](../reports/2026-10-30-completion-inventory.csv) | Closed for the current scope; contextual candidates remain classified, not silently rewritten. |
| Current claims match source and installed help | Guide inspection sections; init/do/mcp-serve/github/gitlab/jira help; CLI chapters 7 and 9 | Corrected residual unconditional MCP wording, chapter count (12), unmeasured on-ramp duration and README framing. External installs remain untested. |
| Product guidance is reachable from book and entry indexes | [Book contents](../SUMMARY.md), [docs index](../README.md), [README](../../README.md), [OVERVIEW](../../OVERVIEW.md), [landing](../showcase/index.html), [library](../showcase/library.html) | Closed: all five entry points reach all nine source/rendered guide pairs, library and executive brief. |
| Current local links resolve; published guides are not orphaned | Refreshed link audit and explicit entry-point traversal | Zero focused current link candidates; nine of nine published guides reachable. Contextual candidates retain their classifications. |
| Report states scope, changes, evidence and unresolved gaps | Original report and this completion delta | Original narrative retained with the explicitly requested overview wording correction and a dated refresh addendum. |
| Lifecycle has an accessible responsive original diagram | [Workflow](../showcase/workflow.html), [source](../product-workflow.md), [SVG](../showcase/assets/workflow.svg) | Closed: title/description, generated alt text and responsive sizing checked. |
| Autonomy has an accessible responsive original diagram and selector | [Autonomy](../showcase/autonomy.html), [source](../selectable-autonomy.md), [SVG](../showcase/assets/autonomy.svg) | Added original diagram; existing selector and its keyboard/no-JavaScript behavior retained. |
| Capabilities have a dedicated linked guide and diagram | [Capabilities](../showcase/capabilities.html), [source](../product-capabilities.md), [SVG](../showcase/assets/capabilities.svg) | Closed: new guide, publication entry, library/search/navigation and diagram. |
| Trust has an accessible responsive original diagram | [Trust](../showcase/trust.html), [source](../trust-and-evidence.md), [SVG](../showcase/assets/trust.svg) | Closed; same-user and authority limits retained. |
| Integrations have a dedicated linked guide and diagram | [Integrations](../showcase/integrations.html), [source](../product-integrations.md), [SVG](../showcase/assets/integrations.svg) | Closed: explicit configuration/credentials/authority distinctions, publication and navigation. |
| Getting started has an accessible responsive original diagram | [Getting started](../showcase/getting-started.html), [source](../getting-started.md), [SVG](../showcase/assets/getting-started.svg) | Added conversational journey diagram; one-time operator setup and opt-in MCP retained. |
| Executive brief and portable site are discoverable | [Executive brief](../showcase/executive-brief.html), [publication README](../showcase/README.md) | Closed for portable files. The separate operator request to publish merged main on GitHub Pages is not a deployment of this unmerged completion branch; the private preview is unchanged. |
| Claims preserve alpha/Linux/authority limits | New guides and existing trust/autonomy/onboarding pages | No invented customer outcomes, ROI or production/security guarantee. Assistive-technology certification is not claimed. |
| PR-3096 finding 1: MCP registration is opt-in | [Approval finding](https://github.com/joemooney/aida/pull/3096#issuecomment-6526785504), first-project and CLI chapter 1 | Verified already closed on merged main: example explicitly uses --with-mcp; chapter 1 states opt-in. Related chapter 7/9 wording corrected here. |
| PR-3096 finding 2: generated-output drift is checked | Builder --check, eight behavioral fixtures, existing CI showcase step | Verified closed on merged main; clean no-write check and fixture suite rerun here. |
| PR-3103 finding 1: dead overview template | [Builder](../showcase/build.py) and publication files | Deleted unreferenced overview-template.html; standalone overview remains separately maintained. |
| PR-3103 finding 2: SVG alt sentence separation | Builder graphic rendering and changed behavioral fixture | Closed: Title. Description. with escaped text; no bare-space run-on. |
| PR-3103 finding 3: false overview preservation wording | [Showcase README](../showcase/README.md), original report correction | Closed: explicitly records standalone Metrics edits and separate maintenance; no claim of byte identity with pre-stack main. |
| PR-3103 finding 4: audit snapshot predates main merge | Refreshed snapshot/inventory/link artifacts above | Re-run against pinned merged main with new guides and book contents in current scope. The dated original snapshot is retained and explicitly superseded. |

## Verification and limits

The publication build and read-only check pass; all eight showcase fixtures pass. An explicit traversal from README, OVERVIEW, docs index, book contents and landing reaches every selected source and generated guide plus library and executive brief. Six required diagrams have SVG title/description/viewBox/role, escaped generated alt text and width:100%/height:auto CSS. The four new SVGs were rendered and visually inspected; no text clipping was found. A headless Chromium screenshot attempt timed out on this host, so no successful new browser-layout or assistive-technology run is claimed. The existing search DOM fixture passes, and the selector code is unchanged.

Refresh the evidence from the repository root after staging added sources. This invokes the existing TASK-HOME-113 auditor with a separate output prefix and excludes self-derived evidence from the hash cycle:

```python
import importlib.util,json,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('audit','docs/audit-documentation.py')
a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
a.PREFIX='2026-10-30-completion'
a.CURRENT.update({'docs/product-capabilities.md','docs/product-integrations.md','docs/SUMMARY.md'})
a.EXCLUDED.update({f'docs/reports/{a.PREFIX}{s}' for s in ('-inventory.csv','-links.csv','-snapshot.json')})
a.EXCLUDED.add('docs/plans/2026-10-30-docs-completion-gap-pass.md')
sys.argv=['audit','--base','79ed6f56bf8f6dc34aaeb43f1b8d232cdf5b82aa'];a.main()
p=a.OUT/(a.PREFIX+'-snapshot.json');s=json.loads(p.read_text());s['date']='2026-10-30';s['scope']='Completion-branch working-tree hashes; Git context is merged main 79ed6f56. Supersedes the 2026-10-29 snapshot for current hashes/counts. Self-derived completion outputs and gap table excluded; historical artifacts retained. Link scan is not semantic, external-URL or accessibility certification.';p.write_text(json.dumps(s,indent=2)+'\n')

for suffix in ('-inventory.csv', '-links.csv'):
    p=a.OUT/(a.PREFIX+suffix);p.write_bytes(p.read_bytes().replace(b'\r\n',b'\n'))
```

Run `python3 docs/showcase/build.py --check`, `python3 tests/test_showcase_build.py`, `git diff --check` and `make check-ci-fast`. Fast-gate completion and spawned author self-check results are recorded in the PR trail, not treated as independent approval. This docs change grants no review, hold-clearance or merge authority.
