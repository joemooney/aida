import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "docs/monitor-contract-fixtures/verify.py"
SPEC = importlib.util.spec_from_file_location("monitor_contract_verify", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)


class MonitorContractCompatibilityTests(unittest.TestCase):
    # trace:STORY-1352 | ai:codex
    def test_removed_field_fails_with_field_name_without_major_bump(self):
        old = {"status": {"fields": {"requirements.total": "integer"}}}
        current = {"status": {"fields": {}}}
        errors = VERIFY.compatibility_errors(old, current, (1, 0, 0), (1, 1, 0))
        self.assertEqual(errors, ["major version bump required for: status.requirements.total"])

    def test_additive_field_requires_minor_bump(self):
        old = {"status": {"fields": {"requirements.total": "integer"}}}
        current = {"status": {"fields": {"requirements.total": "integer", "branch": "string"}}}
        errors = VERIFY.compatibility_errors(old, current, (1, 0, 0), (1, 0, 1))
        self.assertEqual(errors, ["minor version bump required for: status.branch"])

    def test_major_bump_allows_breaking_change(self):
        old = {"status": {"fields": {"requirements.total": "integer"}}}
        current = {"status": {"fields": {}}}
        self.assertEqual(VERIFY.compatibility_errors(old, current, (1, 0, 0), (2, 0, 0)), [])


if __name__ == "__main__":
    unittest.main()
