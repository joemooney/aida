#!/usr/bin/env python3
"""Exercise the shipped status jq projection with GitHub StatusContext rows."""
# trace:BUG-2492 | ai:codex
import json
from pathlib import Path
import re
import subprocess
import unittest


class StatusRollup(unittest.TestCase):
    def test_commit_status_states_survive_projection(self):
        source = (Path(__file__).resolve().parents[1] /
                  'aida-cli-lib/src/status_sections.rs').read_text()
        query, = re.findall(r'r#"(\[\(\.number\|tostring\).*?)"#', source)
        for state, expected in [('SUCCESS', 'SUCCESS'),
                                ('PENDING', 'PENDING,SUCCESS'),
                                ('FAILURE', 'FAILURE,SUCCESS'),
                                ('ERROR', 'ERROR,SUCCESS')]:
            with self.subTest(state=state):
                data = dict(number=1, title='Example', url='https://example.test',
                            state='OPEN', statusCheckRollup=[
                                dict(__typename='CheckRun', conclusion='SUCCESS'),
                                dict(__typename='StatusContext', state=state)])
                output = subprocess.check_output(
                    ['jq', '-r', query], input=json.dumps(data), text=True)
                self.assertEqual(output.rstrip('\n').split('\t')[-1], expected)


if __name__ == '__main__':
    unittest.main()
