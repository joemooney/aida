"""Fixture evidence for the advisor's pre-merge flag check.
trace:STORY-2693 | ai:codex
"""
import importlib.util
from pathlib import Path
import unittest
import json
import subprocess
import sys
import tempfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('flags', Path(__file__).resolve().parents[1] / 'scripts/check-open-pr-flags.py')
flags = importlib.util.module_from_spec(spec)
spec.loader.exec_module(flags)


def diff(body, path='aida-cli-lib/src/doctor.rs'):
    return f'diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n{body}\n'


class OpenPrFlags(unittest.TestCase):
    def test_two_open_prs_report_before_merge(self):
        prs = [dict(number=3662, diff=diff('+#[arg(long = "lane-host")]')),
               dict(number=3643, diff=diff('+Arg::new("host").long("lane-host")'))]
        self.assertEqual(flags.report(prs), [{'flag': '--lane-host', 'prs': [3643, 3662]}])
        self.assertEqual(flags.report(list(reversed(prs))), flags.report(prs))

    def test_implicit_long_and_removed_flag(self):
        self.assertEqual(flags.added_flags(diff('+    #[arg(long)]\n+    lane_host: bool,')), ['--lane-host'])
        self.assertEqual(flags.added_flags(diff('     #[arg(long)]\n-    old_host: bool,\n+    lane_host: bool,')), ['--lane-host'])
        self.assertEqual(flags.added_flags(diff('-#[arg(long = "host")]\n+#[arg(long = "host", help = "new")]')), [])

    def test_fixture_command_reports_to_advisor_without_forge(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / 'prs.json'
            fixture.write_text(json.dumps([
                dict(number=n, diff=diff('+#[arg(long = "lane-host")]'))
                for n in (3643, 3662)]))
            result = subprocess.run([sys.executable, flags.__file__, '--fixture', str(fixture)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            output = json.loads(result.stdout)
            self.assertEqual(output['audience'], 'advisor')
            self.assertEqual(output['checked_prs'], 2)
            self.assertEqual(output['potential_flag_collisions'][0]['prs'], [3643, 3662])

    def test_live_inventory_refuses_a_moving_head(self):
        inventory = json.dumps([dict(number=1, headRefOid='old', isDraft=True, headRefName='feature')])
        with patch.object(flags, 'gh', side_effect=[inventory, diff('+#[arg(long = "host")]'),
                                                  json.dumps(dict(headRefOid='new', state='OPEN'))]):
            with self.assertRaisesRegex(ValueError, 'changed during inspection'):
                flags.live_prs()

    def test_live_inventory_includes_drafts_and_skips_train_copies(self):
        inventory = json.dumps([
            dict(number=1, headRefOid='same', isDraft=True, headRefName='feature'),
            dict(number=2, headRefOid='copy', isDraft=False, headRefName='train/copy')])
        with patch.object(flags, 'gh', side_effect=[inventory, diff('+#[arg(long = "host")]'),
                                                  json.dumps(dict(headRefOid='same', state='OPEN'))]) as gh:
            self.assertEqual([pr['number'] for pr in flags.live_prs()], [1])
            self.assertEqual(gh.call_count, 3)

    def test_non_source_and_duplicate_sites_are_not_duplicate_prs(self):
        self.assertEqual(flags.added_flags(diff('+#[arg(long = "host")]', 'help.txt')), [])
        self.assertEqual(flags.report([dict(number=1, diff=diff('+#[arg(long = "host")]\n+#[arg(long = "host")]'))]), [])


if __name__ == '__main__':
    unittest.main()
