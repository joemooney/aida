#!/usr/bin/env python3
"""Bounded Claude hook payload and handoff transport fixtures."""
# trace:STORY-2662 | ai:codex
import json
import os
import runpy
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / 'aida-core/templates/hooks/aida-precompact-handoff.sh'


class PrecompactHandoff(unittest.TestCase):
    def fixture(self, event='PreCompact', extra=None, fail=False, large=False, slow=False, missing=False, rotated=False, slow_findings=False, pr_fail=False, branch="story-2662", scope="STORY-2662", slow_pr=False, saturated=False, empty_pr=False, findings_response=None, pr_response=None, role="implementer", rotation_seat="", fleet_load=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for flag, enabled in [('slow-findings', slow_findings), ('pr-fail', pr_fail), ('slow-pr', slow_pr), ('saturated', saturated), ('empty-pr', empty_pr), ('fleet-load', fleet_load)]:
                if enabled:
                    (root/flag).touch()
            fake = root / 'aida'
            fake.write_text('''#!/usr/bin/env python3
import os, pathlib, sys, time
p = pathlib.Path(os.environ['FIXTURE'])
args = sys.argv[1:]
with (p/'calls').open('a') as f: f.write(repr(args)+'\\n')
if os.environ['FAIL'] == '1': sys.exit(1)
if '--write' in args:
    (p/'packet').write_text(sys.stdin.read())
elif '--show' in args:
    if '--snapshot' not in args:
        print('MATCHED ROTATION' if '--rotated' in args else 'SIBLING PACKET')
    elif os.environ['MISSING'] == '1': pass
    elif '--body-only' in args: print('Next step: finish parser regression')
    else: print('standing order '*2000 + 'Next step: finish parser regression')
elif 'verdict' in args:
    if os.environ['SLOW'] == '1': time.sleep(20)
    if (p/'fleet-load').exists(): time.sleep(3.5)
    if (p/'slow-pr').exists(): time.sleep(1.66)
    print('Rework: validate parser')
elif 'pr' in args:
    if (p/'slow-pr').exists(): time.sleep(1.97)
    if (p/'pr-fail').exists(): sys.exit(1)
    if 'PR_RESPONSE' in os.environ:
        print(os.environ['PR_RESPONSE'])
        sys.exit(0)
    print(__import__('json').dumps([{'headRefName': os.environ['FIXTURE_BRANCH'], 'number': 3655}] * (0 if (p/'empty-pr').exists() else 100 if (p/'saturated').exists() else 1)))
elif 'findings' in args:
    if (p/'slow-findings').exists(): time.sleep(5.4)
    if (p/'fleet-load').exists(): time.sleep(8.8)
    if 'FINDINGS_RESPONSE' in os.environ:
        print(os.environ['FINDINGS_RESPONSE'])
        sys.exit(0)
    print('{"findings":[{"id":"TASK-1","source":"implementer","origin":"STORY-2662","title":"parser mismatch"},{"id":"TASK-2","source":"review","origin":"PR-3655","title":"review regression"},{"origin":"STORY-other","title":"unrelated finding"}]}' if os.environ['LARGE'] == '0' else '界'*100000)
''')
            fake.chmod(0o755)
            (root / 'gh').symlink_to(fake)
            git = root / 'git'
            git.write_text('#!/bin/sh\nprintf "%s\\n" "$FIXTURE_BRANCH"\n')
            git.chmod(0o755)
            env = dict(PATH=directory+':'+os.defpath,
                       FIXTURE=directory, AIDA_SESSION_ROLE=role,
                       AIDA_ROTATION_SEAT=rotation_seat, AIDA_SESSION_SCOPE=scope, FIXTURE_BRANCH=branch,
                       FAIL=str(int(fail)), LARGE=str(int(large)), SLOW=str(int(slow)),
                       MISSING=str(int(missing)), AIDA_ROTATION_HANDOFF='matched.md' if rotated else '')
            if findings_response is not None:
                env['FINDINGS_RESPONSE'] = json.dumps(findings_response)
            if pr_response is not None:
                env['PR_RESPONSE'] = json.dumps(pr_response)
            payload = dict(hook_event_name=event, cwd=directory, session_id='fixture', trigger='auto')
            payload.update(extra or {})
            result = subprocess.run([str(HOOK)], input=json.dumps(payload), text=True,
                                    capture_output=True, env=env, timeout=15)
            return (result, (root/'packet').read_text() if (root/'packet').exists() else '',
                    (root/'calls').read_text() if (root/'calls').exists() else '')

    def test_snapshot_uses_existing_writer_and_preserves_pointers(self):
        result, packet, calls = self.fixture()
        self.assertEqual(result.returncode, 0)
        for value in ['STORY-2662', 'story-2662', 'Rework', 'parser mismatch', 'review regression', 'Captured:']:
            self.assertIn(value, packet)
        self.assertIn("'--write', '-'", calls)
        self.assertLessEqual(len(packet.encode()), 10000)
        self.assertEqual(result.stdout, '')
        self.assertNotIn('standing order', packet)
        self.assertNotIn('SIBLING PACKET', packet)
        self.assertNotIn('unrelated finding', packet)

    def test_compact_resume_injects_handoff(self):
        result, packet, calls = self.fixture('SessionStart', {'source': 'compact'})
        self.assertIn('finish parser regression', json.loads(result.stdout)['hookSpecificOutput']['additionalContext'])
        self.assertEqual(packet, '')
        self.assertNotIn('--write', calls)

    def test_startup_and_resume_never_inject_shared_latest(self):
        for source in ['startup', 'resume']:
            result, packet, calls = self.fixture('SessionStart', {'source': source})
            self.assertEqual((result.stdout, packet, calls), ('', '', ''))

    def test_snapshot_transport_uses_role_and_internal_worktree_slot(self):
        _, _, calls = self.fixture()
        self.assertIn("'--seat', 'implementer', '--snapshot', '--write', '-'", calls)

    def test_snapshot_never_recursively_embeds_previous_packet(self):
        _, packet, calls = self.fixture()
        self.assertIn('no session-bound prior handoff', packet)
        self.assertNotIn('finish parser regression', packet)
        self.assertNotIn("'--show'", calls)

    def test_every_snapshot_reads_only_matched_rotation_prior(self):
        for missing in [True, False]:
            _, packet, calls = self.fixture(missing=missing, rotated=True)
            self.assertIn('MATCHED ROTATION', packet)
            self.assertNotIn('SIBLING PACKET', packet)
            self.assertIn('--rotated', calls)

    def test_slow_realistic_findings_probe_is_retained(self):
        result, packet, _ = self.fixture(slow_findings=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('parser mismatch', packet)
        self.assertIn('review regression', packet)

    def test_realistic_combined_probe_latency(self):
        result, packet, calls = self.fixture(slow_pr=True, slow_findings=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('review regression', packet)
        self.assertNotIn('Review findings unknown', packet)
        self.assertIn("'--head', 'story-2662'", calls)

    def test_fleet_load_latencies_retain_verdict_and_findings(self):
        import time
        started = time.monotonic()
        result, packet, _ = self.fixture(fleet_load=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('Rework: validate parser', packet)
        self.assertIn('review regression', packet)
        self.assertLess(time.monotonic() - started, 14)

    def test_role_alias_matches_ordinary_handoff_lookup(self):
        for event, extra in [('PreCompact', {}), ('SessionStart', {'source': 'compact'})]:
            _, _, calls = self.fixture(event, extra, role=' DiALog ')
            self.assertIn("'--seat', 'advisor'", calls)
        _, _, calls = self.fixture(role='dialog', rotation_seat='custom')
        self.assertIn("'--seat', 'custom'", calls)

    def test_unscoped_reviewer_uses_pr_pointer_for_verdict_and_next_step(self):
        _, packet, calls = self.fixture(branch='pr-3655', scope='', empty_pr=True)
        self.assertIn('Spec pointer: PR-3655', packet)
        self.assertIn("['review', 'verdict', 'PR-3655']", calls)
        self.assertIn('aida review verdict PR-3655', packet)
        self.assertNotIn('aida show unknown', packet)
        self.assertIn('review regression', packet)

    def test_pr_scope_is_known_even_when_it_matches_spec_origin(self):
        _, packet, _ = self.fixture(scope='PR-3655')
        self.assertIn('review regression', packet)
        self.assertNotIn('Review findings unknown', packet)

    def test_missing_optional_executable_stays_unknown(self):
        run = runpy.run_path(str(HOOK), run_name='fixture')['run']
        self.assertEqual(run(['/no/such/aida-hook-command']), 'unknown (command unavailable)')

    def test_probe_closing_stdout_before_exit_stays_unknown(self):
        run = runpy.run_path(str(HOOK), run_name='fixture')['run']
        # EOF is not successful command completion. Preserve the packet when
        # a probe closes its output stream but hangs before exiting.
        result = run([sys.executable, '-c',
                      'import os,time; os.close(1); time.sleep(20)'], timeout=.2)
        self.assertEqual(result, 'unknown (command timed out)')

    def test_saturated_branch_list_stays_unknown(self):
        _, packet, _ = self.fixture(saturated=True)
        self.assertIn('Review findings unknown', packet)

    def test_empty_branch_lookup_stays_unknown(self):
        _, packet, _ = self.fixture(empty_pr=True)
        self.assertIn('Review findings unknown', packet)

    def test_malformed_pr_rows_never_supply_partial_or_coerced_origins(self):
        valid = {'headRefName': 'story-2662', 'number': 3655}
        for row in [None, [], {'headRefName': None, 'number': 3655},
                    {'headRefName': 'story-2662', 'number': True},
                    {'headRefName': 'story-2662', 'number': '3655'},
                    {'headRefName': 'story-2662', 'number': 3655.5},
                    {'headRefName': 'story-2662', 'number': 0}]:
            with self.subTest(row=row):
                result, packet, _ = self.fixture(pr_response=[valid, row])
                self.assertEqual(result.returncode, 0)
                self.assertIn('Review findings unknown', packet)
                self.assertIn('parser mismatch', packet)
                self.assertNotIn('review regression', packet)

    def test_review_branch_retains_pr_origin_without_head_match(self):
        _, packet, _ = self.fixture(branch='pr-3655', empty_pr=True)
        self.assertIn('review regression', packet)
        self.assertNotIn('Review findings unknown', packet)

    def test_canonical_branch_fallback_labels_spec_findings_unknown(self):
        _, packet, _ = self.fixture(branch='story-b6b1278890774ef993d2ebc327862470-585', scope='')
        self.assertIn('Spec findings unknown', packet)

    def test_failed_pr_lookup_is_explicitly_unknown(self):
        _, packet, _ = self.fixture(pr_fail=True)
        self.assertIn('parser mismatch', packet)
        self.assertIn('Review findings unknown', packet)

    def test_resume_labels_old_packet_after_a_failed_write(self):
        result, _, _ = self.fixture('SessionStart', {'source': 'compact'})
        context = json.loads(result.stdout)['hookSpecificOutput']['additionalContext']
        self.assertIn('may predate the latest compaction', context)

    def test_branch_fallback_does_not_match_inside_words(self):
        for branch in ['debug-parser', 'bug-parser-fix', 'task-alpha', 'bug-123abc']:
            with self.subTest(branch=branch):
                _, packet, _ = self.fixture(branch=branch, scope='')
                self.assertIn('Spec pointer: unknown', packet)
                self.assertNotIn('aida show unknown', packet)

    def test_numeric_and_canonical_branch_pointers(self):
        for branch, pointer in [('bug-123-fix', 'BUG-123'), ('task-1-042-fix', 'TASK-1-042'),
                                ('story-b6b1278890774ef993d2ebc327862470-585', 'STORY-B6B1278890774EF993D2EBC327862470-585')]:
            _, packet, _ = self.fixture(branch=branch, scope='')
            self.assertIn('Spec pointer: ' + pointer, packet)

    def test_large_output_is_bounded(self):
        result, packet, _ = self.fixture(large=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('findings unavailable or exceeded output cap', packet)
        self.assertLessEqual(len(packet.encode()), 10000)

    def test_malformed_findings_remain_unknown_and_packet_still_writes(self):
        for response in [[], {'findings': None}, {'findings': {}}, {'findings': [None]},
                         {'findings': ['invalid row']}, {'findings': [{'origin': None}]}]:
            with self.subTest(response=response):
                result, packet, _ = self.fixture(findings_response=response)
                self.assertEqual(result.returncode, 0)
                self.assertIn('findings unavailable', packet)
                self.assertIn('Next step:', packet)
                self.assertNotIn('Traceback', result.stderr)

    def test_failed_cli_does_not_block_compaction(self):
        result, packet, _ = self.fixture(fail=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(packet, '')
        self.assertIn('unavailable', result.stderr)

    def test_slow_probe_is_unknown_and_packet_still_writes(self):
        result, packet, _ = self.fixture(slow=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('unknown (command timed out)', packet)
        self.assertIn('Next step:', packet)

    def test_oversized_or_malformed_payload_is_a_noop(self):
        for payload in ['{' , 'x' * 65537]:
            result = subprocess.run([str(HOOK)], input=payload, text=True,
                                    capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, '')

    def test_wrong_event_or_invalid_cwd_never_writes(self):
        for event, extra in [('Stop', {}), ('PreCompact', {'cwd': 'relative'}), ('PreCompact', {'session_id': None})]:
            result, packet, calls = self.fixture(event, extra)
            self.assertEqual(result.returncode, 0)
            self.assertEqual((packet, calls), ('', ''))

    def test_claude_only_registration(self):
        settings = json.loads((ROOT/'aida-core/templates/settings.json').read_text())
        self.assertIn('aida-precompact-handoff.sh', settings['hooks']['PreCompact'][0]['hooks'][0]['command'])
        codex = json.loads((ROOT/'aida-core/templates/codex-hooks.json').read_text())
        self.assertNotIn('PreCompact', codex['hooks'])


if __name__ == '__main__':
    unittest.main()
