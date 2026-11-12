#!/usr/bin/env python3
"""Release install and replacement fixtures. trace:STORY-2704 | ai:codex"""
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseInstall(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        # Explicit child inputs keep host configuration out of install fixtures.
        # trace:STORY-2704 | ai:codex
        self.env = {'HOME': str(self.root), 'PATH': '/usr/bin:/bin'}
        self.src = self.root / 'bundle'
        self.dest = self.root / 'installed'
        self.src.mkdir()
        self.dest.mkdir()
        for name in ('aida', 'aida-server', 'aida-porcelain'):
            path = self.src / name
            path.write_text('#!/bin/sh\necho new-' + name + '\n')
            path.chmod(0o755)
        (self.src / 'aida-flow').symlink_to('aida-porcelain')

    def run_upgrade(self, env=None):
        return subprocess.run(['sh', str(ROOT / 'scripts/upgrade-release-bundle.sh'),
                               str(self.src), str(self.dest), 'linux-x86_64'],
                              env=self.env if env is None else env, capture_output=True)

    def assert_bundle(self):
        for name in ('aida', 'aida-server', 'aida-porcelain', 'aida-flow'):
            want = 'aida-porcelain' if name == 'aida-flow' else name
            self.assertEqual(subprocess.check_output([str(self.dest / name)], env=self.env),
                             ('new-' + want + '\n').encode())
        self.assertEqual(os.readlink(self.dest / 'aida-flow'), 'aida-porcelain')

    def test_upgrade_replaces_running_pair_and_companion(self):
        for name in ('aida', 'aida-server', 'aida-porcelain', 'aida-flow'):
            (self.dest / name).write_text('old-' + name)
        # An open executable handle must keep the old inode, never truncation.
        with (self.dest / 'aida').open() as old:
            result = self.run_upgrade()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(old.read(), 'old-aida')
        self.assert_bundle()

    def test_missing_member_refuses_before_replacement(self):
        (self.dest / 'aida').write_text('old-aida')
        (self.src / 'aida-server').unlink()
        self.assertNotEqual(self.run_upgrade().returncode, 0)
        self.assertEqual((self.dest / 'aida').read_text(), 'old-aida')

    def test_historical_pair_remains_installable(self):
        (self.src / 'aida-flow').unlink()
        (self.src / 'aida-porcelain').unlink()
        result = self.run_upgrade()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(subprocess.check_output([str(self.dest / 'aida')], env=self.env), b'new-aida\n')
        self.assertTrue((self.dest / 'aida-server').is_file())

    def test_replacement_failure_rolls_back(self):
        # Exercise every publication boundary, including the link replacement
        # after all three binaries have moved. trace:STORY-2704 | ai:codex
        for failed_member in ('aida', 'aida-server', 'aida-porcelain', 'aida-flow'):
            with self.subTest(failed_member=failed_member):
                for name in ('aida', 'aida-server', 'aida-porcelain'):
                    (self.dest / name).write_text('old-' + name)
                link = self.dest / 'aida-flow'
                link.unlink(missing_ok=True)
                link.symlink_to('old-porcelain')
                bin_dir = self.root / ('bin-' + failed_member)
                bin_dir.mkdir()
                # Fail only publication; rollback uses the old/ paths.
                stub = bin_dir / 'mv'
                stub.write_text('#!/bin/sh\ncase "$2" in */new/' + failed_member
                                + ') exit 1;; esac\nexec /usr/bin/mv "$@"\n')
                stub.chmod(0o755)
                env = dict(self.env, PATH=str(bin_dir) + ':' + self.env['PATH'])
                self.assertNotEqual(self.run_upgrade(env).returncode, 0)
                for name in ('aida', 'aida-server', 'aida-porcelain'):
                    self.assertEqual((self.dest / name).read_text(), 'old-' + name)
                self.assertEqual(os.readlink(link), 'old-porcelain')
                self.assertEqual(list(self.dest.glob('.aida-upgrade-*')), [])

    def test_release_archive_contains_multicall_link(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        step = workflow.split('- name: Package Unix release tarball', 1)[1]
        script = step.split('run: |\n', 1)[1].split('\n      - name:', 1)[0]
        script = '\n'.join(line[10:] for line in script.splitlines())
        script = script.replace('${{ matrix.name }}', 'fixture').replace(
            '${{ matrix.target }}', 'fixture-target')
        release = self.root / 'target/fixture-target/release'
        release.mkdir(parents=True)
        for name in ('aida', 'aida-server', 'aida-porcelain'):
            (release / name).write_bytes((self.src / name).read_bytes())
        subprocess.run(['bash', '-eu', '-c', script], cwd=self.root, env=self.env, check=True)
        with tarfile.open(self.root / 'fixture.tar.gz') as tar:
            self.assertEqual(set(tar.getnames()),
                             {'aida', 'aida-server', 'aida-porcelain', 'aida-flow'})
            self.assertTrue(tar.getmember('aida-flow').issym())
            self.assertEqual(tar.getmember('aida-flow').linkname, 'aida-porcelain')

    def test_make_install_roots(self):
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        cargo = bin_dir / 'cargo'
        cargo.write_text('#!/bin/sh\n[ "$1 $2" = "install --path" ] || exit 2\n'
                         '[ "$4 $5" = "--force --root" ] || exit 3\n'
                         'case $3 in aida-cli) name=aida;; *) name=$3;; esac\n'
                         'mkdir -p "$6/bin"\n'
                         'cp "$FIXTURE_BUNDLE/$name" "$6/bin/$name"\n')
        cargo.chmod(0o755)
        for target in ('install', 'install-cli'):
            for custom_root in (False, True):
                with self.subTest(target=target, custom_root=custom_root):
                    home = self.root / (target + str(custom_root)) / 'cargo home'
                    root = home.parent / 'custom root' if custom_root else home
                    env = dict(self.env, PATH=str(bin_dir) + ':' + self.env['PATH'],
                               CARGO_HOME=str(home), FIXTURE_BUNDLE=str(self.src))
                    if custom_root:
                        env['CARGO_INSTALL_ROOT'] = str(root)
                    result = subprocess.run(['make', '-o', 'build-release', target],
                                            cwd=ROOT, env=env, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.dest = root / 'bin'
                    self.assertEqual(os.readlink(self.dest / 'aida-flow'), 'aida-porcelain')
                    for name in ('aida', 'aida-porcelain', 'aida-flow'):
                        self.assertTrue((self.dest / name).is_file())
                    if target == 'install':
                        self.assert_bundle()
                    if custom_root:
                        self.assertFalse(home.exists())

    def test_install_script_temp_prefix(self):
        archive = self.root / 'release.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            for path in self.src.iterdir():
                tar.add(path, arcname=path.name)
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        curl = bin_dir / 'curl'
        curl.write_text('#!/bin/sh\n[ "$1 $2" = "-fSL -o" ] || exit 2\ncp "$FIXTURE_ARCHIVE" "$3"\n')
        curl.chmod(0o755)
        env = dict(self.env, PATH=str(bin_dir) + ':' + self.env['PATH'],
                   FIXTURE_ARCHIVE=str(archive))
        result = subprocess.run(['bash', str(ROOT / 'scripts/install.sh'), '--version',
                                 'v1.0.0', '--prefix', str(self.dest)], env=env,
                                capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_bundle()


if __name__ == '__main__':
    unittest.main()
