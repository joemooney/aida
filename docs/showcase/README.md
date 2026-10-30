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

## Source guide build

`python3 docs/showcase/build.py` renders the selected source guide into the
shared `page-template.html` layout. Edit `docs/getting-started.md`, the template
or `guide.css`, then rebuild; do not hand-edit `getting-started.html`.
The Python standard library is sufficient. The renderer intentionally supports
only the Markdown used by these guides, with explicit page selection in `PAGES`.
Generated files are committed and can be read directly from disk.
