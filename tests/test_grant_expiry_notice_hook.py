#!/usr/bin/env python3
"""Claude PreToolUse expiry notice protocol, without a compiled CLI."""
# trace:BUG-2014 | ai:codex
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / 'aida-core/templates/hooks/aida-grant-expiry-notice.sh'
MESSAGE = 'The advisor seat grant expired. Renew: aida role enter advisor'
PAYLOAD = {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'additionalContext': MESSAGE}}


class NoticeHook(unittest.TestCase):
    # trace:BUG-2014 | ai:codex
    def test_settings_hooks_are_installed_and_tracked(self):
        result = subprocess.run(
            ["python3", str(ROOT / "scripts/check-template-hooks.py")],
            text=True, capture_output=True, cwd=ROOT,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def run_hook(self, body, grant='fixture'):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            fake = path / 'aida'
            fake.write_text('#!/bin/sh\n' + body)
            fake.chmod(0o755)
            env = dict(os.environ, PATH=str(path) + ':' + os.environ['PATH'], AIDA_SESSION_GRANT=grant)
            result = subprocess.run(['sh', str(HOOK)], input=json.dumps({'hook_event_name': 'PreToolUse', 'tool_name': 'Bash', 'tool_input': {'command': 'true'}}), text=True, capture_output=True, env=env, timeout=3)
            return result

    def test_no_grant_never_spawns_cli(self):
        for grant in ['', None]:
            if grant is None:
                with tempfile.TemporaryDirectory() as directory:
                    fake = Path(directory) / 'aida'
                    fake.write_text('#!/bin/sh\necho SPAWNED\nexit 9\n')
                    fake.chmod(0o755)
                    env = dict(os.environ, PATH=directory + ':' + os.environ['PATH'])
                    env.pop('AIDA_SESSION_GRANT', None)
                    result = subprocess.run(['sh', str(HOOK)], env=env, text=True, capture_output=True, timeout=3)
            else:
                result = self.run_hook('echo SPAWNED\nexit 9\n', grant)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, '', ''))

    def test_real_hook_json_without_user_prompt(self):
        result = self.run_hook("[ \"$*\" = 'role expiry-notice' ] || exit 5\nprintf '%s\\n' '" + json.dumps(PAYLOAD) + "'\n")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), PAYLOAD)
        self.assertEqual(result.stderr, '')
        self.assertNotIn('permissionDecision', result.stdout)

    def test_ledger_failure_is_silent_fail_open(self):
        result = self.run_hook("echo 'Expiry notice unavailable: invalid ledger' >&2\nexit 0\n")
        self.assertEqual((result.returncode, result.stdout), (0, ''))
        self.assertEqual(len(result.stderr.splitlines()), 1)

    def test_failed_command_discards_partial_context(self):
        result = self.run_hook("echo PARTIAL\nexit 7\n")
        self.assertEqual((result.returncode, result.stdout), (0, ''))
        self.assertEqual(len(result.stderr.splitlines()), 1)

    def test_hard_timeout_below_two_seconds(self):
        started = time.monotonic()
        result = self.run_hook('echo PARTIAL\nsleep 10\n')
        elapsed = time.monotonic() - started
        self.assertLess(elapsed, 2.1)
        self.assertEqual((result.returncode, result.stdout), (0, ''))
        self.assertEqual(len(result.stderr.splitlines()), 1)

    def test_every_tool_registration_and_early_store_free_dispatch(self):
        settings = json.loads((ROOT / 'aida-core/templates/settings.json').read_text())
        matches = [entry for entry in settings['hooks']['PreToolUse'] if any('aida-grant-expiry-notice.sh' in hook['command'] for hook in entry['hooks'])]
        self.assertEqual(len(matches), 1)
        self.assertNotIn('matcher', matches[0])
        self.assertEqual(matches[0]['hooks'][0]['timeout'], 2)
        source = (ROOT / 'aida-cli-lib/src/lib_part1.rs').read_text()
        dispatch = source.index('return role_cmd::handle_role_command(role_cmd);')
        # Pin the actual early return and unreachable late dispatch contract.
        storage = source.index('let storage =', dispatch)
        self.assertLess(dispatch, storage)
        self.assertLess(dispatch, source.index('role is dispatched before storage init'))
        role = (ROOT / 'aida-cli-lib/src/role_cmd.rs').read_text()
        self.assertLess(role.index('return crate::grant_expiry_notice::handle();'), role.index('let project_root = statusline_project_root();'))
        notice = (ROOT / 'aida-cli-lib/src/grant_expiry_notice.rs').read_text()
        self.assertNotIn('Storage::', notice)
        self.assertNotIn('cache.db', notice.split('#[cfg(test)]')[0])


if __name__ == '__main__':
    unittest.main()
