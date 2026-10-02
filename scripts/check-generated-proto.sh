#!/usr/bin/env bash
set -euo pipefail

# Generated-protobuf drift guard: the two committed protobuf mirrors must be
# byte-identical to what the build scripts produce from proto/. When they drift,
# every worktree build leaves an unrelated dirty file and the next agent has to
# decide whether to commit it. trace:TASK-1567 | ai:claude
#
# The guard DELETES both mirrors and regenerates them rather than regenerating
# over them. A compare-in-place check passes vacuously whenever codegen does not
# run at all -- and codegen for the CLI mirror is behind `--features remote`,
# which is NOT in that crate's default set, so "forgot the feature" is the
# likeliest way this gate would go quietly dead. Deleting first turns that
# failure from a silent pass into a missing file.

cd "$(dirname "$0")/.."

PROTO=proto/aida.proto
MIRRORS=(
  aida-server/src/generated/aida.rs
  aida-cli-lib/src/generated/aida.rs
)

# --- Preconditions -----------------------------------------------------------
# Each mirror must exist AND be tracked. An untracked or deleted mirror would
# otherwise make the final `git diff` report no drift -- absent is not matching.
for f in "${MIRRORS[@]}"; do
  if [[ ! -f $f ]]; then
    echo "error: generated mirror is missing: $f" >&2
    echo "       it must be committed, not generated on demand" >&2
    exit 1
  fi
  if ! git ls-files --error-unmatch "$f" >/dev/null 2>&1; then
    echo "error: generated mirror is not tracked by git: $f" >&2
    exit 1
  fi
done

if [[ ! -f $PROTO ]]; then
  echo "error: $PROTO is missing; nothing to generate from" >&2
  exit 1
fi

# Pre-existing edits to a mirror would be indistinguishable from drift below.
if ! git diff --quiet -- "${MIRRORS[@]}"; then
  echo "error: a generated mirror already has uncommitted edits:" >&2
  git diff --name-only -- "${MIRRORS[@]}" >&2
  echo "       commit or revert it before running this guard" >&2
  exit 1
fi

# --- Force a real regeneration ----------------------------------------------
# Both build scripts declare `rerun-if-changed=../proto/aida.proto`, so touching
# the proto is what guarantees they re-run; deleting the outputs alone would not.
echo "==> regenerating the protobuf mirrors from $PROTO"
rm -f "${MIRRORS[@]}"
touch "$PROTO"

# aida-server generates unconditionally; aida-cli-lib only with `remote`.
cargo check -p aida-server
cargo check -p aida-cli-lib --features remote

# --- Proof that codegen actually ran ----------------------------------------
missing=()
for f in "${MIRRORS[@]}"; do
  [[ -f $f ]] || missing+=("$f")
done
if (( ${#missing[@]} > 0 )); then
  echo "error: codegen did not reproduce these mirrors:" >&2
  printf '       %s\n' "${missing[@]}" >&2
  echo "       the build script that writes them did not run -- check the" >&2
  echo "       feature flags above against aida-cli-lib/Cargo.toml" >&2
  # Leave the tree usable: restore what we deleted.
  git checkout -- "${MIRRORS[@]}" 2>/dev/null || true
  exit 1
fi

# --- The drift verdict -------------------------------------------------------
if ! git diff --quiet -- "${MIRRORS[@]}"; then
  echo "error: the committed protobuf mirrors are stale" >&2
  git --no-pager diff --stat -- "${MIRRORS[@]}" >&2
  echo >&2
  git --no-pager diff -- "${MIRRORS[@]}" >&2
  echo >&2
  echo "       the regenerated files are in your working tree; commit them:" >&2
  echo "         git add ${MIRRORS[*]}" >&2
  exit 1
fi

echo "ok: both protobuf mirrors match what proto/aida.proto generates"
