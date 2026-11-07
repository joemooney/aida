#!/usr/bin/env python3
"""Round-trip the shipped notice hook's JSON through both common awk engines.

The original corruption did not reproduce on mawk 1.3.4 20240123 or gawk
5.2.1; these tests verify portable escaping, not a version-specific failure.
"""
# trace:BUG-2475 | ai:codex
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / 'aida-core/templates/hooks/aida-mail-notice.sh'


class MailNoticeHook(unittest.TestCase):
    def run_hook(self, engine, notice, hello, client='agy', event='SessionStart'):
        executable = shutil.which(engine)
        self.assertIsNotNone(executable, f'{engine} is required for this regression test')
        with tempfile.TemporaryDirectory(prefix='mail-notice-') as directory:
            root = Path(directory)
            (root / 'awk').symlink_to(executable)
            (root / 'notice').write_text(notice)
            (root / 'hello').write_text(hello)
            fake = root / 'aida'
            fake.write_text('''#!/bin/sh
case "$*" in
    'awaiting --notice'|'mailbox notice') cat "$AIDA_SESSION_PROJECT/notice" ;;
    'mailbox hello --hook') cat "$AIDA_SESSION_PROJECT/hello" ;;
    'schedule tick --hook') exit 0 ;;
    *) exit 1 ;;
esac
''')
            fake.chmod(0o755)
            env = {'PATH': directory + ':/usr/bin:/bin',
                   'AIDA_SESSION_PROJECT': directory, 'HOME': directory}
            result = subprocess.run(
                ['sh', str(HOOK), '--client', client, '--event', event],
                input='{}', text=True, capture_output=True, env=env, timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stderr, '')
            return result.stdout

    def test_json_round_trip(self):
        notice = 'Paths: C:\\temp\\new\\file \\b \\r \\u0041 \\"quote"\t雪\nsecond line\\\n'
        hello = 'Hello: "advisor"\t\\server\\share\\\n'
        for engine in ('mawk', 'gawk'):
            for event in ('SessionStart', 'PreInvocation'):
                with self.subTest(engine=engine, event=event):
                    output = json.loads(self.run_hook(engine, notice, hello, event=event))
                    self.assertEqual(output, {'injectSteps': [{'systemMessage': {
                        'systemMessage': (notice + hello).removesuffix('\n'),
                    }}]})

    def test_empty_notice_and_cr_normalization(self):
        for engine in ('mawk', 'gawk'):
            with self.subTest(engine=engine):
                self.assertEqual(json.loads(self.run_hook(engine, '', '')), {})
                output = json.loads(self.run_hook(engine, 'notice\r\n\r\n', 'hello\r\n'))
                self.assertEqual(output['injectSteps'][0]['systemMessage']['systemMessage'],
                                 'notice\n\nhello')
                # Other clients retain the plain-text relay.
                self.assertEqual(self.run_hook(engine, 'notice\\t\n', 'hello\t\n', client='claude'),
                                 'notice\\t\nhello\t\n')


if __name__ == '__main__':
    unittest.main()
