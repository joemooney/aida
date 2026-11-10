# Lane Rust cache reuse investigation

<!-- trace:BUG-2519 | ai:codex -->

The existing lane-host doctor cache check proves that the job sandbox can
contact its private server and write the shared disk cache. It does not prove
that a compilation in another seat can reuse those entries.

On the dirtlab host, sccache 0.19.0 reproduced a working-directory-dependent
cache key without Cargo or the primary host's Cargo slot wrapper:

- Compile a trivial rlib with `--emit=dep-info,link` in directory A.
- Compile it in directory B, holding the absolute source, output directory,
  compiler, crate name and flags constant.
- Compile again in A as a positive control.

The result was three executed Rust requests, two misses and one hit, with no
cache errors. A separate A/B/A probe with identical copies of the source and
`SCCACHE_BASEDIRS` naming both roots also produced two misses and one hit.
This establishes a path-sensitive cache miss mechanism on this host; it does
not establish the primary host's full dependency hit rate or exclude other
causes there. Different target seeding and which dependencies Cargo actually
recompiles can also affect the two hosts' reported rates.

`aida doctor --lane-host` now adds advisory `lane-cache-reuse` evidence. Its
bounded sandbox probe holds source and output paths constant while changing
only cwd. A same-cwd retry distinguishes path-sensitive misses from failure to
cache at all. Fresh random crate/source names prevent older probe artifacts
from supplying false hits; the sandbox's server statistics are private.
This probe does not invoke Cargo, modify its target or bypass its slot gate.
A failed performance probe grants no admission or containment authority.

Upstream references:

- https://github.com/mozilla/sccache/blob/main/docs/Rust.md
- https://github.com/mozilla/sccache/issues/2652

## Remaining gate

The primary host and its reviewed codegraph client are unavailable in this
seat. Acceptance still requires two comparable primary lane dependency builds
with a second-job Rust hit rate of at least 50%; no such result is claimed.

A stable sandbox-visible workspace/target layout could eliminate differing
absolute paths, but requires a reviewed mount-policy design covering Git
metadata protection, Cargo manifest/config discovery, build-script outputs,
debug paths and continued seat isolation. Alternatively, evaluate an upstream
sccache change with equivalent artifact-path safety guarantees. Simply setting
`SCCACHE_BASEDIRS` did not repair the local reproduction.

Keep this bug open pending that design decision and primary-host measurement.
