"""Fake-only exec profile/argv probe; no operator config, credentials or inference.

trace:BUG-2487 | ai:codex
"""
import hashlib
import http.server
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile
import threading


def bounded(argv, env, cwd, timeout):
    child = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             start_new_session=True)
    try:
        out, err = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGKILL)
        out, err = child.communicate()
        raise AssertionError('synthetic exec profile probe timed out: ' +
                             (out + err)[-4000:].decode(errors='replace')) from None
    return subprocess.CompletedProcess(argv, child.returncode, out, err)


def probe(binary, args, preflight):
    fixture = Path(__file__).resolve().parents[2] / 'launch' / 'codex_0166_help.json'
    recorded = json.loads(fixture.read_text())
    with Path(binary).open('rb') as executable:
        assert hashlib.file_digest(executable, 'sha256').hexdigest() == recorded['sha256']
    # HOME and CODEX_HOME start empty. All writable inputs belong to this probe.
    with tempfile.TemporaryDirectory(prefix='aida-2487-', dir='/tmp') as tmp:
        root = Path(tmp)
        for name in ('home', 'runtime', 'allowed', 'denied', 'daemon'):
            (root / name).mkdir(mode=0o700)
        env = dict(HOME=str(root / 'home'), CODEX_HOME=str(root / 'home'),
                   TMPDIR=str(root / 'runtime'), XDG_RUNTIME_DIR=str(root / 'runtime'),
                   PATH='/usr/bin:/bin', LANG='C.UTF-8',
                   HTTP_PROXY='http://127.0.0.1:9', HTTPS_PROXY='http://127.0.0.1:9',
                   NO_PROXY='127.0.0.1')
        for scope in ('exec', 'sandbox'):
            result = bounded([binary, scope, '--help'], env, tmp, 5)
            assert result.returncode == 0 and result.stdout.decode() == recorded[scope]
        # Exercise the production builder, including its -c overrides, in the
        # real exec parser before setting up the synthetic provider.
        result = bounded([binary, *args, '--help'], env, tmp, 5)
        assert result.returncode == 0, result.stderr.decode()

        # The parent native sandbox masks Codex's shared daemon directory with
        # mode 000. A nested namespace substitutes an EMPTY private 0700 root;
        # this never exposes the host daemon. Give nested bwrap its own proc/dev.
        wrapper = ['/usr/bin/bwrap', '--unshare-user', '--ro-bind', '/', '/',
                   '--proc', '/proc', '--dev', '/dev', '--bind', tmp, tmp,
                   '--bind', str(root / 'daemon'), f'/tmp/codex-daemon-{os.getuid()}', '--']
        allowed = root / 'allowed' / 'yes'
        denied = root / 'denied' / 'no'
        writes = (f'from pathlib import Path; Path({str(allowed)!r}).write_text("yes"); '
                  f'Path({str(denied)!r}).write_text("no")')
        # Both paths are writable in the outer boundary. Only Codex's selected
        # profile distinguishes them in the subsequent model tool call.
        control = bounded([*wrapper, '/usr/bin/python3', '-c', writes], env, tmp, 5)
        assert control.returncode == 0 and allowed.exists() and denied.exists()
        allowed.unlink()
        denied.unlink()
        observations = []

        class Recorder(http.server.BaseHTTPRequestHandler):
            def log_message(self, *unused):
                pass

            def do_POST(self):
                self.connection.settimeout(2)
                length = int(self.headers['Content-Length'])
                assert 0 < length <= 1048576, 'synthetic request exceeds bound'
                request = json.loads(self.rfile.read(length))
                observations.append(request)
                assert len(observations) <= 2, 'unexpected synthetic model loop'
                if len(observations) == 1:
                    assert any(t.get('name') == 'exec_command' for t in request['tools'])
                    item = dict(type='function_call', id='fc_probe', call_id='call_probe',
                                name='exec_command', arguments=json.dumps(dict(
                                    cmd='/usr/bin/python3 -c ' + shlex.quote(writes),
                                    max_output_tokens=1000)))
                else:
                    item = dict(type='message', id='msg_probe', role='assistant',
                                status='completed', content=[dict(type='output_text',
                                text='probe complete', annotations=[])])
                response = dict(id=f'resp_{len(observations)}', status='completed',
                                output=[item], usage=dict(input_tokens=1, output_tokens=1,
                                                         total_tokens=2))
                events = [dict(type='response.created', response=dict(
                    id=response['id'], status='in_progress', output=[])),
                    dict(type='response.output_item.added', output_index=0, item=item),
                    dict(type='response.output_item.done', output_index=0, item=item),
                    dict(type='response.completed', response=response)]
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                for event in events:
                    self.wfile.write(('event: ' + event['type'] + '\ndata: ' +
                                      json.dumps(event) + '\n\n').encode())
                self.wfile.flush()

        # Ephemeral listener, not a persistent application port.
        with http.server.HTTPServer(('127.0.0.1', 0), Recorder) as server:
            threading.Thread(target=server.serve_forever, daemon=True).start()
            config = dict(model_provider='fixture', model='gpt-5.4',
                **{'model_providers.fixture.name': 'fixture',
                   'model_providers.fixture.base_url': f'http://127.0.0.1:{server.server_port}/v1',
                   'model_providers.fixture.wire_api': 'responses',
                   'model_providers.fixture.requires_openai_auth': False,
                   'model_providers.fixture.request_max_retries': 0,
                   'model_providers.fixture.stream_max_retries': 0,
                   'features.responses_websockets': False,
                   'features.responses_websockets_v2': False,
                   'approval_policy': 'never'})
            # Same directory-based format as WriteProfile::native(). Selection
            # comes ONLY from the production argv passed in by the Rust test.
            filesystem = {'/': 'read', **{str(root / n): 'write'
                          for n in ('home', 'runtime', 'allowed')}}
            (root / 'home' / 'config.toml').write_text(
                '[permissions.aida.filesystem]\n' + ''.join(
                    json.dumps(k) + ' = ' + json.dumps(v) + '\n'
                    for k, v in filesystem.items()) +
                '[permissions.aida.network]\nenabled = false\n')
            seat = bounded([*wrapper, *preflight, '/usr/bin/python3', '-c', writes],
                           env, tmp, 5)
            assert seat.returncode != 0 and allowed.exists() and not denied.exists(), (
                'sandbox preflight profile failed: ' + seat.stderr.decode())
            diagnostic = seat.stdout + seat.stderr
            assert b'Read-only file system' in diagnostic or b'Permission denied' in diagnostic, (
                'missing sandbox denial diagnostic: ' + diagnostic.decode())
            allowed.unlink()
            argv = [binary, *args[:-1]]
            for key, value in config.items():
                argv.extend(['-c', key + '=' + json.dumps(value)])
            argv.append(args[-1])
            try:
                result = bounded([*wrapper, *argv], env, tmp, 40)
            finally:
                server.shutdown()
        assert result.returncode == 0, result.stderr.decode()
        assert allowed.read_text() == 'yes', 'positive write control failed'
        assert not denied.exists(), 'exec ignored selected profile'
        outputs = [item.get('output', '') for request in observations
                   for item in request.get('input', [])
                   if item.get('type') == 'function_call_output']
        assert any('Read-only file system' in out or 'Permission denied' in out
                   for out in outputs), 'missing denied-write tool evidence'
        print(json.dumps(dict(version=recorded['version'], sha256=recorded['sha256'],
            exec_argv_accepted=True, sandbox_preflight_write=True, outer_write_control=True, allowed_write=True,
            denied_write=True, selection='default_permissions=aida', synthetic_only=True)))


if __name__ == '__main__':
    probe(sys.argv[1], json.loads(sys.argv[2]), json.loads(sys.argv[3]))
