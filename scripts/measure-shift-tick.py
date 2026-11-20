#!/usr/bin/env python3
"""Host/cron acceptance probe for BUG-2626; runs a real, enabled shift tick."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--root", type=Path, required=True, help="primary checkout used by cron")
parser.add_argument("--binary", type=Path, required=True, help="deployed aida executable")
parser.add_argument("--deployed-sha", required=True, help="source SHA attested by the host deployment")
args = parser.parse_args()
root = args.root.resolve(strict=True)
binary = args.binary.resolve(strict=True)
ledger = root / ".aida/gate-ledger"
files = list(ledger.glob("PR-*.jsonl")) + list(ledger.glob("MR-*.jsonl"))
binary_digest = hashlib.sha256()
with binary.open("rb") as executable:
    for chunk in iter(lambda: executable.read(1024 * 1024), b""):
        binary_digest.update(chunk)
evidence = {
    "spec": "BUG-2626",
    "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "root": str(root),
    "binary": str(binary),
    "deployed_sha": args.deployed_sha,
    "binary_sha256": binary_digest.hexdigest(),
    "change_ledgers": len(files),
    "ledger_bytes": sum(p.stat().st_size for p in files),
    "by_spec_entries": len(list((ledger / "by-spec").iterdir())),
    "argv": [str(binary), "shift", "tick", "--json"],
}
started = time.monotonic()
try:
    result = subprocess.run(evidence["argv"], cwd=root, capture_output=True, text=True, timeout=120)
    evidence.update(exit_code=result.returncode, stdout=result.stdout, stderr=result.stderr)
    try:
        report = json.loads(result.stdout)
    except (ValueError, TypeError):
        report = {}
    # Disabled/lock-contended ticks return enabled=false, ran=false. A real
    # report includes dry_run=false; a fast refusal is not acceptance evidence.
    ran = isinstance(report, dict) and report.get("enabled") is True and report.get("dry_run") is False
    evidence["non_lock_contended_tick"] = ran
    evidence["hold_discovery_refused"] = "shift hold discovery refused:" in result.stderr
except subprocess.TimeoutExpired:
    evidence.update(exit_code=124, non_lock_contended_tick=False, hold_discovery_refused=False, error="120s budget exhausted")
evidence["elapsed_seconds"] = time.monotonic() - started
evidence["accepted"] = (
    evidence["exit_code"] == 0
    and evidence["non_lock_contended_tick"]
    and not evidence["hold_discovery_refused"]
    and evidence["elapsed_seconds"] < 120
)
print(json.dumps(evidence, sort_keys=True))
raise SystemExit(0 if evidence["accepted"] else 1)
