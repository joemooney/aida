"""Fake-only installed-Codex compatibility check, called by the Rust test.

Consumes the production reduced memfd, never the operator's login. This is
interface evidence only, not containment or successful inference evidence.
trace:STORY-2593 | ai:codex
"""
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
import tempfile
import threading


def bounded(argv, env, timeout, cwd=None):
    # Keep the group leader unreaped until communicate completes; kill the
    # private group on timeout so wrapper descendants cannot retain pipes.
    child = subprocess.Popen(argv, env=env, cwd=cwd, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, start_new_session=True)
    try:
        out, err = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGKILL)
        child.communicate()
        raise AssertionError('synthetic Codex probe timed out') from None
    return subprocess.CompletedProcess(argv, child.returncode, out, err)


def probe(binary, fd):
    raw = os.pread(fd, 65537, 0)
    auth = json.loads(raw)
    access = auth['tokens']['access_token']
    payload = access.split('.')[1]
    claims = json.loads(base64.urlsafe_b64decode(payload + '=' * (-len(payload) % 4)))
    assert claims.get('synthetic_access_sentinel') == '2593', 'fake-only probe requires sentinel'
    assert auth['tokens']['refresh_token'] == '', 'refresh material refused'
    observations = []

    class Recorder(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            observations.append(dict(path=self.path,
                bearer_matches=self.headers.get('Authorization') == 'Bearer ' + access,
                account_matches=self.headers.get('ChatGPT-Account-ID') == 'account-sentinel'))
            self.rfile.read(min(int(self.headers.get('Content-Length', '0')), 1048576))
            self.send_response(400)
            self.end_headers()
            self.wfile.write(b'{"error":{"message":"synthetic fixture stop"}}')

        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"models":[]}')

        def log_message(self, *args):
            pass

    with tempfile.TemporaryDirectory(prefix='aida-2593-probe-') as tmp:
        home = Path(tmp)
        codex = home / 'codex'
        codex.mkdir()
        leaf = codex / 'auth.json'
        leaf.write_bytes(raw)
        leaf.chmod(0o600)
        env = dict(PATH='/usr/bin:/bin', HOME=tmp, CODEX_HOME=str(codex),
                   XDG_CONFIG_HOME=str(home / 'config'), XDG_CACHE_HOME=str(home / 'cache'),
                   XDG_DATA_HOME=str(home / 'data'),
                   HTTP_PROXY='http://127.0.0.1:9', HTTPS_PROXY='http://127.0.0.1:9',
                   ALL_PROXY='http://127.0.0.1:9', NO_PROXY='127.0.0.1', LANG='C.UTF-8')
        version_result = bounded([binary, '--version'], env, 3)
        assert version_result.returncode == 0, 'version probe refused'
        version = version_result.stdout.decode().strip()
        assert version == 'codex-cli 0.167.0', 're-probe this codex version'
        # Ephemeral listener, not a persistent application port.
        with http.server.HTTPServer(('127.0.0.1', 0), Recorder) as server:
            server.timeout = 2
            threading.Thread(target=server.serve_forever, daemon=True).start()
            config = {'model_provider': 'fixture', 'model': 'fixture-model',
                'model_providers.fixture.name': 'fixture',
                'model_providers.fixture.base_url': f'http://127.0.0.1:{server.server_port}/v1',
                'model_providers.fixture.wire_api': 'responses',
                'model_providers.fixture.requires_openai_auth': True,
                'model_providers.fixture.request_max_retries': 0,
                'model_providers.fixture.stream_max_retries': 0,
                'features.responses_websockets': False,
                'features.responses_websockets_v2': False,
                'cli_auth_credentials_store': 'file'}
            argv = [binary, 'exec', '--skip-git-repo-check', '--json']
            for key, value in config.items():
                argv.extend(['-c', key + '=' + json.dumps(value)])
            argv.append('Reply fixture only')
            try:
                result = bounded(argv, env, 8, cwd=tmp)
            finally:
                server.shutdown()
        assert result.returncode == 1, 'expected fixture HTTP 400'
        assert any(r == dict(path='/v1/responses', bearer_matches=True, account_matches=True)
                   for r in observations), 'exact leaf did not authenticate and route'
        assert access.encode() not in result.stdout + result.stderr, 'token in CLI output'
        assert leaf.read_bytes() == raw, 'CLI changed credentials'
        # Scan private runtime artifacts, without printing contents or secrets.
        for path in home.rglob('*'):
            if path.is_file() and path != leaf:
                assert path.stat().st_size <= 8 * 1024 * 1024, 'probe artifact exceeds scan bound'
                assert access.encode() not in path.read_bytes(), 'token in CLI runtime artifact'
        print(json.dumps(dict(version=version, executable_sha256=hashlib.sha256(
            Path(binary).read_bytes()).hexdigest(), bearer_matches=True, account_matches=True,
            fixture_http_status=400, auth_unchanged=True, secret_artifacts=False)))


if __name__ == '__main__':
    probe(sys.argv[1], int(sys.argv[2]))
