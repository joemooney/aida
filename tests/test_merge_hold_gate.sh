#!/usr/bin/env bash
set -euo pipefail

gate() {
  LABELS=$1 bash -c '
    has_label() { printf "%s" "$LABELS" | tr "," "\n" | grep -Fqx "$1"; }
    has_label aida:merge-hold && exit 1
    if has_label aida:merge-hold-recorded && ! has_label aida:merge-hold-cleared; then exit 1; fi
    if has_label aida:merge-hold-cleared && ! has_label aida:merge-hold-recorded; then exit 1; fi
  '
}

# Hand-removing the marker leaves the active forge label and still fails.
if gate 'aida:merge-hold,aida:merge-hold-recorded'; then
  echo 'FAIL: marker deletion released a held PR' >&2
  exit 1
fi

# Hand-removing the active label leaves history without clearance and still fails.
if gate 'aida:merge-hold-recorded'; then
  echo 'FAIL: label deletion released a held PR' >&2
  exit 1
fi

# Only the recorded clearance state releases the historical hold.
gate 'aida:merge-hold-recorded,aida:merge-hold-cleared'
gate ''
if gate 'aida:merge-hold-cleared'; then
  echo 'FAIL: clearance without a recorded hold passed' >&2
  exit 1
fi
echo 'merge-hold gate fixtures passed'
