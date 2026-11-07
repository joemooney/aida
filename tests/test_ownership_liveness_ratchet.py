"""Inventory remaining legacy lease readers; new sites belong in ownership/."""
# trace:STORY-2325 | ai:codex
import importlib.util
import json
from collections import Counter
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("exec_context_tokens", ROOT / "scripts/check-exec-context.py")
LEX = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LEX)
BASELINE = ROOT / "scripts/ownership-liveness-legacy.json"
PROBES = {
    "lease_state_for", "lease_is_live", "lease_process_verdict", "lease_owner_gone",
    "lease_uses_holder_verdict", "auto_release_decision_for_lease",
    "stale_lease_recovery_for_lease", "stale_lease_recovery_for_lease_with",
    "lease_owner_process_gone", "process_identity_is_alive", "pid_is_alive",
    "process_provably_dead", "probe_live_claude_sessions",
}


def sites(source):
    tokens = LEX.tokens(source)
    aliases = {}
    # A reference/import counts as a site too. Aliased calls cannot hide one.
    for i, token in enumerate(tokens):
        if token in PROBES and tokens[i + 1:i + 2] == ["as"] and i + 2 < len(tokens):
            aliases[tokens[i + 2]] = token
    result = Counter()
    scopes, pending, depth = [], None, 0
    for i, token in enumerate(tokens):
        if token == "fn" and i + 1 < len(tokens):
            pending = tokens[i + 1]
        if token == "{":
            depth += 1
            if pending:
                scopes.append((depth, pending))
                pending = None
        elif token == "}":
            if scopes and scopes[-1][0] == depth:
                scopes.pop()
            depth -= 1
        elif token == ";":
            pending = None
        probe = aliases.get(token, token)
        if probe in PROBES and (i == 0 or tokens[i - 1] != "fn"):
            scope = scopes[-1][1] if scopes else "<module>"
            result[f"{scope}:{probe}"] += 1
    return result


def inventory():
    result = {}
    for path in sorted((ROOT / "aida-cli-lib/src").rglob("*.rs")):
        rel = path.relative_to(ROOT).as_posix()
        if "/ownership/" in rel or "/tests/" in rel:
            continue
        for site, count in sites(path.read_text()).items():
            result[f"{rel}:{site}"] = count
    return result


class OwnershipLivenessRatchet(unittest.TestCase):
    def test_no_new_legacy_sites(self):
        baseline = json.loads(BASELINE.read_text())
        growth = {key: count for key, count in inventory().items() if count > baseline.get(key, 0)}
        self.assertFalse(growth, f"Route new liveness readers through ownership/: {growth}")

    def test_comments_literals_aliases_and_item_movement(self):
        self.assertEqual(sites('fn f() { /* pid_is_alive */ let s = "lease_is_live"; }'), {})
        found = sites('use p::lease_state_for as inspect; fn f() { inspect(l); }')
        self.assertEqual(found["f:lease_state_for"], 1)
        self.assertNotEqual(sites('fn f() { lease_state_for(l); }'), sites('fn g() { lease_state_for(l); }'))
        self.assertEqual(sites('fn f() { lease_is_live(l); lease_is_live(l); }')["f:lease_is_live"], 2)


if __name__ == "__main__":
    unittest.main()
