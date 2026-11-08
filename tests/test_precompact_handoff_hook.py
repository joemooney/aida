#!/usr/bin/env python3
"""Bounded Claude hook payload and handoff transport fixtures."""
# trace:STORY-2662 | ai:codex
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / 'aida-core/templates/hooks/aida-precompact-handoff.sh'


class PrecompactHandoff(unittest.TestCase):
    def fixture(self, event='PreCompact', extra=None, fail=False, large=False, slow=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / 'aida'
            fake.write_text('''#!/usr/bin/env python3
import os, pathlib, sys, time
p = pathlib.Path(os.environ['FIXTURE'])
args = sys.argv[1:]
with (p/'calls').open('a') as f: f.write(repr(args)+'\\n')
if os.environ['FAIL'] == '1': sys.exit(1)
if '--write' in args:
    (p/'packet').write_text(sys.stdin.read())
elif '--show' in args: print('Next step: finish parser regression')
elif 'verdict' in args:
    if os.environ['SLOW'] == '1': time.sleep(20)
    print('Rework: validate parser')
elif 'findings' in args:
    print('finding: parser mismatch' if os.environ['LARGE'] == '0' else '界'*100000)
''')
            fake.chmod(0o755)
            git = root / 'git'
            git.write_text('#!/bin/sh\nprintf "story-2662\\n"\n')
            git.chmod(0o755)
            env = dict(os.environ, PATH=directory+':'+os.environ['PATH'],
                       FIXTURE=directory, AIDA_SESSION_ROLE='implementer',
                       AIDA_ROTATION_SEAT='', AIDA_SESSION_SCOPE='STORY-2662',
                       FAIL=str(int(fail)), LARGE=str(int(large)), SLOW=str(int(slow)))
            payload = dict(hook_event_name=event, cwd=directory, session_id='fixture', trigger='auto')
            payload.update(extra or {})
            result = subprocess.run([str(HOOK)], input=json.dumps(payload), text=True,
                                    capture_output=True, env=env, timeout=15)
            return (result, (root/'packet').read_text() if (root/'packet').exists() else '',
                    (root/'calls').read_text() if (root/'calls').exists() else '')

    def test_snapshot_uses_existing_writer_and_preserves_pointers(self):
        result, packet, calls = self.fixture()
        self.assertEqual(result.returncode, 0)
        for value in ['STORY-2662', 'story-2662', 'Rework', 'parser mismatch', 'finish parser regression']:
            self.assertIn(value, packet)
        self.assertIn("'--write', '-'", calls)
        self.assertLessEqual(len(packet.encode()), 10000)
        self.assertEqual(result.stdout, '')

    def test_compact_resume_injects_handoff(self):
        result, packet, calls = self.fixture('SessionStart', {'source': 'compact'})
        self.assertIn('finish parser regression', json.loads(result.stdout)['hookSpecificOutput']['additionalContext'])
        self.assertEqual(packet, '')
        self.assertNotIn('--write', calls)

    def test_large_output_is_bounded(self):
        result, packet, _ = self.fixture(large=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('[truncated]', packet)
        self.assertLessEqual(len(packet.encode()), 10000)

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
        for event, extra in [('Stop', {}), ('PreCompact', {'cwd': 'relative'})]:
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
