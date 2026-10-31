# AIDA product showcase

A presentation for newcomers and engineering management, centered on **a governed software factory with selectable autonomy**.

- Open [index.html](index.html) for the interactive showcase.
- Open [executive-brief.html](executive-brief.html) for a concise meeting handout. Its Print / Save as PDF button uses the browser's print dialog.
- Read [the detailed overview](../what-is-aida.md) for supporting technical context and limitations.

The site is buildless and self-contained: HTML, CSS, JavaScript and an inline SVG diagram. It has no external font, package, analytics or image dependency. The landing page and brief work when opened directly from disk. To preserve the guide’s source and reference links, serve the repository layout with `docs/showcase/index.html` as the entry, or provide those linked docs alongside it; copying only this directory does not include the references. Publishing to a Git repository alone does not make the HTML a website.

The autonomy selector supports mouse, touch, arrow keys, Home and End. With JavaScript disabled, the initial autonomous-delivery explanation remains readable. Print styles are included, and reduced-motion preferences are respected.

The diagrams explain the operating model; they are not a live dashboard. Descriptive delivery labels map to existing execution modes rather than introducing new CLI values. Claims are grounded in `ExecutionMode` in `aida-core/src/models.rs`, the human contracts in `aida-cli-lib/src/do_dispatch.rs`, and the existing overview. Keep the alpha/Linux and authority limitations accurate when editing the copy.

The initial version was published as an owner-private preview through Sites. The hosted prototype may contain later, unmerged work; the source in this directory defines this bounded showcase. Access is separate from the portable files, and sharing a private preview URL does not make it public. Hosting identity and credentials are not part of this directory.

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-101 | ai:codex -->

## Public GitHub Pages site

Open **[joemooney.github.io/aida](https://joemooney.github.io/aida/)** for the live website.
The operator requested public Pages hosting on 30 October 2026. The initial
publication is a snapshot of reviewed merged main `79ed6f56`; it does not
publish unmerged follow-up changes. Pages deploys from the separate `gh-pages`
branch at `/`, with `.nojekyll`. It does not automatically follow `main`.
Refresh that deployment from an approved merged revision when the guides change.
Only static showcase pages/assets and the standalone overview are published;
source and technical-reference links resolve to the public GitHub repository.
The existing owner-private Sites preview retains its separate audience.

<!-- trace:TASK-HOME-130 | ai:codex -->

### Assembling the published site

`python3 docs/showcase/publish.py --rev <revision> --out <empty-dir>` produces
the published file set from one git revision: the static pages, styles,
scripts and assets in this directory (not templates, `build.py`,
`publication.json` or this README), `docs/what-is-aida.html` as
`overview.html`, and `.nojekyll`. Links to published files are rewritten to
their published names. All other repository links point at the public GitHub
repository. The script uses only Python's standard library and Git, and makes
no network calls. Unchanged inputs give byte-identical output.

To refresh GitHub Pages from an approved merged revision, check out
`gh-pages` in a separate worktree, replace its files with the output of
`publish.py --rev <revision>`, and commit with the revision in the message.

## GitLab Pages site

The `pages` job in `.gitlab-ci.yml` publishes the same site to GitLab Pages.
It runs `publish.py` for the pipeline's revision into `public/`, which GitLab
serves. Like `gh-pages`, it publishes only from an approved merged revision.
It never runs on mirrored pushes or merge requests. It runs in two cases:

- **A release tag** (`vN…`) mirrored to GitLab.
- **A manual run.** Open *Build > Pipelines > Run pipeline* on the default
  branch and set the variable `PUBLISH_PAGES` to `true`. The job waits for that
  pipeline's verify and test stages.

The site is served at `https://<namespace>.<pages-domain>/<project>/`, for
example `https://joe.pages.example.com/aida/`. *Deploy > Pages* in the project
shows the exact URL. Pages does not work on a self-hosted instance until its
administrator enables it: set `pages_external_url` (the Pages domain) in
`gitlab.rb` and add a wildcard DNS record `*.<pages-domain>` pointing at the
instance. Wildcard TLS is needed for HTTPS. Until then the job can run, but
nothing is served. That is a blocker for the instance operator, and there is
no workaround inside this repository.

Inside an air gap, github.com is unreachable. Run the job with
`--repo-url <gitlab-project-url> --blob-prefix <gitlab-project-url>/-/blob/main/`
added to both `publish.py` lines to point source links at the local project.
This deliberately makes that site differ from GitHub Pages.

### Confirming both hosts match

Both hosts publish identical files when they publish the same revision with
the default options. To confirm:

1. Read the revision each host serves: the `gh-pages` commit message, and the
   commit of the GitLab `pages` job (*Deploy > Pages*, or the job page).
2. Check GitHub: `git worktree add $TMPDIR/ghp origin/gh-pages`, then
   `python3 docs/showcase/publish.py --rev <revision> --compare $TMPDIR/ghp`.
3. Check GitLab: download the `pages` job's `public` artifact, unzip it, then
   run `python3 docs/showcase/publish.py --rev <revision> --compare <unzipped>/public`.

`--compare` lists every missing, extra or differing file. It exits non-zero on
any difference. The `pages` job runs the same comparison on its own output
before publishing.

<!-- trace:TASK-2604 | ai:claude -->

## Source guide build

`python3 docs/showcase/build.py` renders every manifest-selected guide into the
shared `page-template.html` layout: `getting-started.html`, `configuration.html`,
`workflow.html`, `autonomy.html`, `trust.html`, `metrics.html`, `setup.html`,
`capabilities.html` and `integrations.html`,
plus `library.html` and `search-data.js`. Edit the corresponding Markdown,
templates or `guide.css`, then rebuild; do not hand-edit derived outputs.
Python 3 with its standard library and Git are required. The renderer intentionally supports
only the Markdown used by these guides, with explicit page selection in `publication.json`.
Generated files are committed and can be read directly from disk.
Run `python3 docs/showcase/build.py --check` to report every drifted generated
page and search artifact without writing files; the fast CI gate runs this check.

## Curated library and publication scope

`publication.json` is the explicit source allowlist and audience classification.
It currently selects nine tracked product guides, classified as human or operator
reading. Technical references remain links; historical, internal, archived,
plan/spike/security/agent material and runtime data are not republished or indexed.
The manifest keeps publication scope explicit; adding a source requires checking its audience, currentness and references.

`build.py` derives the selected pages, `library.html` and `search-data.js` from
those tracked Markdown sources and the shared templates. No timestamps or Git
revision strings enter the outputs; unchanged inputs produce unchanged outputs.
Each search record includes its source content hash. Comment traces are stripped
from indexed text. Do not hand-edit generated HTML or search data. To deliberately
add a source, review its scope/currentness, stage the Markdown file so Git knows
it, add it to the manifest and rebuild. Merely placing a file in docs does not
publish it. The script uses Python's standard library and the Git executable for tracked-source checks; it performs no network IO.

`library-template.html`, `page-template.html`, `guide.css`, `library.js` and the
manifest are editable masters. Search uses a local script payload rather than
fetch, so it works from disk. Without JavaScript, every selected page and source
link remains listed. This work neither deploys the site nor changes the existing
owner-private hosted preview's audience. Plain Markdown references require the
repository layout described above; they are source references, not generated
handbook pages.

## Standalone overview

`docs/what-is-aida.html` is outside this publication builder. Maintain that standalone file separately, preserving its table semantics, section labels, anchors and nested navigation when updating its content. The guide/library build and `--check` do not regenerate or certify that page.
