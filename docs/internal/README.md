# Working on AIDA

Contributor material for developing AIDA itself. If you are adopting AIDA for
another project, use the [adopter guide](../README.md#im-using-aida).

- [Contributing](../../CONTRIBUTING.md) and [repository guide](agents/aida-repository-guide.md)
- [Architecture](architecture/), [core API contract](api/aida-core.md), [monitor contract](monitor-contract.md)
  and [contract fixtures](monitor-contract-fixtures/)
- [Implementation plans](../plans/) and [normative project decisions](../aida/05-decisions/)
- [Agent discipline and validation](agents/), [testing evidence](testing/)
  and [conformance evidence](conformance/)
- [Research](research/), [spikes](spikes/), [reports](reports/) and [analytics](analytics/)
- [Positioning](positioning/README.md), [competitive analysis](competitive-analysis/README.md),
  [presentations](presentation/README.md) and [future vision](future-vision.md)
- [Security](security/), [explanations](explanations/), [demos](demos/)
  and [release notes](release-notes/)
- [Skills conventions](skills-convention.md), [user-facing text conventions](user-facing-text-conventions.md)
  and [git verbs](git-verb-surface.md)

## Documentation maintenance

<!-- trace:STORY-1651 | ai:codex -->

Adopter guides retain their URLs under `docs/`; operator guides live in
`docs/advanced/`; contributor material lives here. `docs/plans/` remains at its
established path and is indexed above, as do normative ADRs under `docs/aida/`.
Template masters, generated mirrors and live agent instructions keep their
existing locations. The public book navigation contains adopter and advanced
guides; this index and the contributor-only CLI chapter are excluded. Publish
with `python3 scripts/build-reader-book.py` (also used by `make book`): it builds
`docs/book.toml` and removes contributor/archive assets that mdBook otherwise
copies even when they are absent from the navigation. Raw `mdbook build docs`
is a preview, not the publication command.

The cleanup inventory assigns every tracked Markdown/HTML document one audience:
`adopter`, `advanced`, `internal`, `archive` or `generated`. After adding or moving
a document, stage it and regenerate the inventory:

```sh
python3 docs/archive/2026-10-cleanup/inventory.py
python3 docs/archive/2026-10-cleanup/inventory.py --check
make check-ci-fast
```

In a native Codex lane with a read-only Git index, use `--working-tree` when
regenerating and checking pending files before the host stages the changes.
The CI gate checks the tracked file list and rejects missing, duplicate,
stale or invalid audience entries.

Historical records retain their original claims; path retargeting does not
certify their content as current. Retired material remains in [the archive](../archive/README.md).
