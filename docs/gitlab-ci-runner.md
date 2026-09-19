# GitLab mirror CI runner — host setup

The self-hosted GitLab mirror (`gitlab.joemooney.com`, project `ai/aida`) runs
its pipeline on a single docker-executor runner. Since STORY-1216 the pipeline
is **incremental**: nothing is downloaded per job, the checkout persists
between jobs, and cargo's target dir lives on the host. This page is the
host-side recipe that `.gitlab-ci.yml` assumes.

## 1. Builder image (`ci/Dockerfile`)

Toolchain, `clippy` + `rustfmt`, `cargo-nextest`, the cross targets and the apt packages the
workspace needs are baked into one image. Build it on the runner host:

```bash
sudo podman build -t localhost/aida-ci:1.92 -f ci/Dockerfile ci/
```

`.gitlab-ci.yml` references `localhost/aida-ci:1.92` and the runner is
configured with `pull_policy = ["if-not-present"]`, so the image is used from
the host's podman store — no registry, no per-run pull. When you change the
Dockerfile (new Rust pin, new apt package) **bump the tag** in both places so
a stale local image is never reused silently.

## 2. Persistent checkout + cargo cache

Two host directories are mounted into every job container:

| Host path                     | In container | Purpose |
|-------------------------------|--------------|---------|
| `/var/cache/aida-ci/builds`   | `/builds`    | the git checkout persists → `GIT_STRATEGY: fetch` is a fetch, not a clone |
| `/var/cache/aida-ci/cache`    | `/ci-cache`  | `CARGO_HOME` (registry) + `CARGO_TARGET_DIR` persist → incremental builds |

Create them once:

```bash
sudo mkdir -p /var/cache/aida-ci/builds /var/cache/aida-ci/cache
```

The GitLab `cache:` block is gone — the zip/unzip round-trip of a multi-GB
`target/` was itself minutes per job.

## 3. Runner config (`/etc/gitlab-runner/config.toml`)

```toml
concurrent = 2

[[runners]]
  executor = "docker"
  [runners.docker]
    image = "localhost/aida-ci:1.92"
    pull_policy = ["if-not-present"]
    volumes = ["/cache",
               "/var/cache/aida-ci/builds:/builds",
               "/var/cache/aida-ci/cache:/ci-cache"]
```

`concurrent = 2` on purpose: the box has 6 cores, and more slots only pile up
cold builds (the 2026-09-18 validation hit load 45 with eight slots). Each slot
pairs its persistent checkout with `/ci-cache/target-$CI_CONCURRENT_ID`.
Keeping targets slot-local matters: Cargo fingerprints contain source paths,
so sharing one target between the runner's `concurrent-0` and `concurrent-1`
checkout roots made the slots repeatedly recompile each other's crates. The
separate `verify` and `test` jobs can use both slots without that churn.

The pipeline sets `GIT_CLEAN_FLAGS: none`. The checkout and Cargo target are
deliberately persistent, and cleaning the checkout caused Cargo to recompile
unchanged workspace crates after GitLab refreshed their mtimes. Build outputs
are in `/ci-cache/target-*`, not the checkout. If the runner workspace ever needs
a clean reset, stop the runner and clean that project's directory explicitly
rather than putting an unconditional clean back on every pipeline.

GitLab Runner still performs a forced checkout even without `git clean`, which
can refresh tracked-file mtimes. Before each builder job,
`ci/restore-git-mtimes` assigns every regular tracked file a deterministic past
mtime derived from its Git blob ID. Unchanged content therefore keeps the same
mtime across pipelines, while a content change gets a different mtime and
continues to invalidate Cargo correctly. The helper also normalizes `.git/HEAD`
and `.git/index` from the commit and tree IDs because the CLI build-stamp script
intentionally watches those paths; a retry of one SHA remains warm, while a new
commit still refreshes the embedded build identity.

Gotchas learned the hard way:

- `gitlab-runner register` run as your user writes `~/.gitlab-runner/config.toml`;
  the systemd service reads `/etc/gitlab-runner/config.toml`. Register with
  `sudo`, or copy the `[[runners]]` block across. The service hot-reloads.
- Docker on this host is the podman shim (`/var/run/docker.sock` →
  `/run/podman/podman.sock`); the docker executor works because the service
  runs as root. There is no `docker` group.
- A push to the mirror's `main` starts a branch pipeline that competes with MR
  pipelines; the project has "auto-cancel redundant pipelines" on so a new push
  to an MR cancels the previous run of that MR.
- The merge-hold gate job (`aida-merge-hold-gate`, included from
  `aida-core/templates/gitlab-ci-merge-hold-gate.yml`) is alpine-based with
  `needs: []` and no cache, so it runs in seconds regardless of the above.

<!-- trace:STORY-1216 | ai:claude -->
<!-- trace:TASK-1274 | ai:codex -->
