#!/usr/bin/env python3
"""The installer fails loudly on a failed download. trace:TASK-3954 | ai:claude

A stub `curl` on PATH stands in for the network, so each case is a
failed, empty or cut-off download. Every case must exit non-zero, name the
URL, and leave the install directory empty.
"""
import io
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
INSTALL = ROOT / 'scripts/install.sh'
ASSET = 'https://github.com/joemooney/aida/releases/download/v1.0.0/aida-linux-x86_64.tar.gz'

# Prints curl's own error for a blocked proxy and exits as curl does.
FAILING_CURL = '#!/bin/sh\necho "curl: (56) CONNECT tunnel failed, response 403" >&2\nexit 56\n'


class InstallDownloadFailure(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.prefix = self.root / 'bin'
        self.stubs = self.root / 'stubs'
        self.stubs.mkdir()
        self.env = {'HOME': str(self.root), 'TMPDIR': str(self.root),
                    'PATH': str(self.stubs) + ':/usr/bin:/bin'}

    def stub_curl(self, body):
        curl = self.stubs / 'curl'
        curl.write_text(body)
        curl.chmod(0o755)

    def run_installer(self, *extra):
        return subprocess.run(['bash', str(INSTALL), '--version', 'v1.0.0',
                               '--prefix', str(self.prefix), *extra],
                              cwd=self.root, env=self.env, capture_output=True, text=True)

    def assert_failed_loudly(self, result, url=ASSET):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(url, result.stderr)
        self.assertIn('Nothing was installed', result.stderr)
        self.assertNotIn('installed ' + str(self.prefix), result.stdout)
        self.assertEqual(list(self.prefix.glob('*')) if self.prefix.exists() else [], [])

    def test_curl_failure_names_the_url_and_the_offline_route(self):
        self.stub_curl(FAILING_CURL)
        result = self.run_installer()
        self.assert_failed_loudly(result)
        self.assertIn('the download failed', result.stderr)
        self.assertIn('--archive', result.stderr)

    def test_empty_download_is_refused(self):
        self.stub_curl('#!/bin/sh\n: > "$3"\n')
        result = self.run_installer()
        self.assert_failed_loudly(result)
        self.assertIn('the download was empty', result.stderr)

    def test_cut_off_archive_is_refused_before_installing(self):
        data = io.BytesIO()
        with tarfile.open(fileobj=data, mode='w:gz') as tar:
            payload = os.urandom(64 * 1024)
            info = tarfile.TarInfo('aida')
            info.size, info.mode = len(payload), 0o755
            tar.addfile(info, io.BytesIO(payload))
        partial = self.root / 'partial.tar.gz'
        partial.write_bytes(data.getvalue()[: len(data.getvalue()) // 2])
        self.stub_curl('#!/bin/sh\ncp "$PARTIAL" "$3"\n')
        self.env['PARTIAL'] = str(partial)
        result = self.run_installer()
        self.assert_failed_loudly(result)
        self.assertIn('incomplete', result.stderr)

    def test_an_error_page_saved_as_the_archive_is_refused(self):
        self.stub_curl('#!/bin/sh\necho "<html>403 Forbidden</html>" > "$3"\n')
        self.assert_failed_loudly(self.run_installer())

    def test_offline_archive_installs_without_the_network(self):
        archive = self.root / 'aida-linux-x86_64.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            for name in ('aida', 'aida-server'):
                body = ('#!/bin/sh\necho ' + name + '\n').encode()
                info = tarfile.TarInfo(name)
                info.size, info.mode = len(body), 0o755
                tar.addfile(info, io.BytesIO(body))
        self.stub_curl(FAILING_CURL)
        for host in ('github', 'gitlab'):
            with self.subTest(host=host):
                result = subprocess.run(['bash', str(INSTALL), '--host', host, '--prefix',
                                         str(self.prefix), '--archive', archive.name],
                                        cwd=self.root, env=self.env, capture_output=True,
                                        text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn('CONNECT tunnel failed', result.stderr)
                self.assertTrue((self.prefix / 'aida-server').is_file())

    def test_missing_offline_archive_is_refused(self):
        result = self.run_installer('--archive', 'missing.tar.gz')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('missing.tar.gz is missing or empty', result.stderr)
        self.assertFalse(self.prefix.exists())

    def test_documented_command_exits_non_zero_when_the_script_download_fails(self):
        # Run the exact command the README tells a new user to paste.
        readme = (ROOT / 'README.md').read_text()
        command = re.search(r'^curl -fsSL \S+/install\.sh .*$', readme, re.M).group(0)
        self.stub_curl(FAILING_CURL)
        result = subprocess.run(['bash', '-c', command], cwd=self.root, env=self.env,
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('CONNECT tunnel failed', result.stderr)
        self.assertFalse(self.prefix.exists())

    def test_a_cut_off_installer_script_runs_nothing(self):
        lines = INSTALL.read_text().splitlines(keepends=True)
        for cut in (len(lines) // 3, len(lines) // 2, len(lines) * 3 // 4):
            with self.subTest(cut=cut):
                self.stub_curl('#!/bin/sh\necho "curl called" >&2\nexit 0\n')
                result = subprocess.run(['bash', '-s', '--', '--prefix', str(self.prefix)],
                                        input=''.join(lines[:cut]), cwd=self.root,
                                        env=self.env, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('Downloading', result.stdout)
                self.assertNotIn('curl called', result.stderr)
                self.assertFalse(self.prefix.exists())


if __name__ == '__main__':
    unittest.main()
