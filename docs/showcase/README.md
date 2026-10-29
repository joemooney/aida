# AIDA product site

A professional product introduction for newcomers and engineering management, centered on **a governed software factory with selectable autonomy**. People communicate intent; agents normally operate the CLI/MCP. Configurability is a primary product theme.

## Pages

- [Home](index.html): positioning and product map.
- [The product](workflow.html): the human/agent loop, delivery lifecycle and data layers.
- [Autonomy](autonomy.html): the four delivery modes and separate decision stop.
- [Configure](configure.html): six configuration layers, interactive explorer and illustrative policy requests.
- [Trust](trust.html): authority, independent review, evidence and current limits.
- [Metrics](metrics.html): reporting capabilities, coverage, derivable measures and unsupported claims.
- [Start](start.html): human-first adoption journey, with operator setup kept separate.
- [Documentation](resources.html): searchable integrated reference library.
- [Executive brief](executive-brief.html): printable meeting handout.

## Build the documentation views

From the repository root, with Python 3 and pandoc installed:

```bash
python3 docs/showcase/build.py
```

The build renders current/reference Markdown into ignored `handbook/` pages and a content-search index. It excludes archived material, plans, research, dated investigations, presentation decks and other evidence collections. Source links remain available. The generated library is a derived view, not a second documentation authority.

To produce a standalone publish directory:

```bash
python3 docs/showcase/build.py --output /tmp/aida-product-site-dist
```

The resulting output has no runtime framework, package, font, analytics or external image dependency. It can be opened directly from disk or served by any static host. Publishing these files in Git alone does not host a website. Top-level generated pages and original SVG graphics are committed for direct browsing; the large reference library is regenerated rather than duplicated in Git.

## Editing

Edit `pages/*.html` for product-page bodies; `build.py` supplies shared navigation, footer and original SVG diagrams. The authored home page, shared CSS and JavaScript are kept here. Autonomy content on the dedicated page reuses the home selector. Edit the underlying repository Markdown for handbook content, then rebuild. Keep the source-link branch constant in the generator aligned with the published source branch.

Controls support keyboard navigation, mobile menu use and copy-request feedback. Reduced-motion and print styles are present. Illustrative policy requests are not live settings, and diagrams are operating models rather than a live dashboard.

The hosted version remains owner-private in Sites. Hosting access is separate from the portable sources; a preview URL is not public access. Credentials and hosting identity are managed outside this repository.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
