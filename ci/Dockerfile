# AIDA CI builder image for the self-hosted GitLab runner (STORY-1216).
#
# Everything the .gitlab-ci.yml jobs used to apt-get / rustup on EVERY run is
# baked in here once, so a job starts compiling immediately. Rebuild when the
# pinned Rust version or the apt set changes:
#
#   sudo podman build -t localhost/aida-ci:1.92 -f ci/Dockerfile ci/
#
# The runner pulls with `pull_policy = ["if-not-present"]`, so the image is
# used straight from the host's podman store — no registry, no per-run pull.
# Bump the tag (and `image:` in .gitlab-ci.yml) when you change this file so
# a stale local image is never silently reused.
FROM docker.io/library/rust:1.92

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      protobuf-compiler \
      libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
      libxkbcommon-dev libssl-dev libgtk-3-dev \
      bubblewrap \
      gcc-aarch64-linux-gnu curl ca-certificates git \
 && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt \
 && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu

# cargo-nextest: faster test runner (one process per test, better parallelism
# and isolation for env-sensitive tests). Prebuilt binary, pinned by the
# nextest release channel; bump the image tag when changing it.
RUN curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C /usr/local/cargo/bin \
 && cargo nextest --version

ENV CARGO_TERM_COLOR=always
