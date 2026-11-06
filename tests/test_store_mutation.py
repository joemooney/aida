"""S1 protocol, kernel-credential transport and lifecycle refusal fixtures.
No vendor, real grant, mutable canonical store or production mutation handler.
trace:TASK-3225 | ai:codex
"""
import array
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'aida-cli-lib/src'
ns = {}
for name in ('containment/egress_probe.py', 'containment/contained_lane.py',
             'store_mutation/service.py', 'store_mutation/client.py', 'containment/contained_host.py'):
    exec(compile((SOURCE / name).read_text(), name, 'exec'), ns)
Service, Refusal = ns['StoreMutationSession'], ns['MutationRefusal']
parse = ns['mutation_request']


class Identity:
    def __init__(self):
        self.allowed = {os.getpid()}
        self.calls = []
        self.closed = False

    def check(self, pid, uid, gid, sender_fd=None):
        self.calls.append((pid, uid, gid))
        return not self.closed and pid in self.allowed and uid == os.getuid() and gid == os.getgid()

    def close(self):
        self.closed = True


class StoreMutationTests(unittest.TestCase):
    def setUp(self):
        self.identity = Identity()
        self.error = None
        self.auth_calls = 0
        self.epoch = str(uuid.uuid4())
        self.binding = dict(epoch=self.epoch, specs=[str(uuid.uuid4())], role='implementer')
        self.service = Service(self.binding, self.identity, self.authorize)
        self.addCleanup(self.service.close)
        self.credentials = (os.getpid(), os.getuid(), os.getgid())

    def authorize(self):
        self.auth_calls += 1
        return self.error

    def request(self, op='session_probe', data=None, **kwargs):
        return dict(version=1, launch_epoch=self.epoch, request_id=str(uuid.uuid4()),
                    op=op, data={'challenge': 'probe'} if data is None else data, **kwargs)

    def wire(self, request):
        return json.dumps(request).encode()

    def dispatch(self, request):
        return json.loads(self.service.dispatch(self.wire(request), self.credentials))

    def refuses(self, error, request):
        with self.assertRaisesRegex(Refusal, '^' + error + '$'):
            self.dispatch(request)

    def test_closed_operations_and_schemas(self):
        for op in ('own_claim', 'comment_add', 'spec_edit', 'queue_done', 'exec', 'unknown'):
            self.refuses('UnsupportedOperation', self.request(op))
        for field in ('path', 'cwd', 'store', 'program', 'argv', 'git_config', 'replacement', 'role', 'grant', 'pid'):
            req = self.request()
            req['data'][field] = '/host/canary'
            self.refuses('InvalidRequest', req)
            req = self.request()
            req[field] = '/host/canary'
            self.refuses('InvalidRequest', req)
        self.assertFalse(self.service.accepted)

    def test_malformed_duplicate_trailing_and_non_utf8(self):
        raw = self.wire(self.request())
        for packet in (b'', b'[]', b'null', b'\xff', raw + b'{}', raw[:-1] + b',"op":"session_probe"}',
                       raw.replace(b'"probe"', b'NaN'), raw.replace(b'"probe"', b'"\\u0000"')):
            with self.assertRaises(Refusal):
                parse(packet)

    def test_version_ids_bounds_depth_and_types(self):
        for key, value, error in [('version', 2, 'UnsupportedVersion'), ('version', True, 'InvalidRequest'),
                                  ('launch_epoch', 'x', 'InvalidRequest'), ('request_id', '../x', 'InvalidRequest'),
                                  ('data', [], 'InvalidRequest')]:
            req = self.request()
            req[key] = value
            self.refuses(error, req)
        for challenge in ('x' * 129, 'x' * 16385, '\x00', {'path': '/x'}):
            self.refuses('InvalidRequest', self.request(data={'challenge': challenge}))
        with self.assertRaises(Refusal):
            parse(b'x' * 65537)
        req = self.request()
        req['data'] = {str(i): i for i in range(33)}
        self.refuses('InvalidRequest', req)
        req['data'] = {'challenge': [[[[['deep']]]]]}
        for _ in range(20):
            req['data'] = {'x': req['data']}
        self.refuses('InvalidRequest', req)

    def test_replay_conflict_canonical_digest_and_epoch_scope(self):
        req = self.request()
        first = self.dispatch(req)
        replay = json.loads(self.service.dispatch(json.dumps(req, sort_keys=True).encode(), self.credentials))
        self.assertEqual(first, replay)
        self.assertEqual(len(self.service.accepted), 1)
        req['data']['challenge'] = 'different'
        self.refuses('Conflict', req)
        req = self.request()
        req['launch_epoch'] = str(uuid.uuid4())
        self.refuses('Unauthorized', req)
        self.assertEqual(len(self.service.accepted), 1)

    def test_status_unknown_is_not_success_of_target(self):
        probe = self.request()
        self.dispatch(probe)
        for target, state in ((probe['request_id'], 'completed'), (str(uuid.uuid4()), 'unknown')):
            result = self.dispatch(self.request('request_status', {'request_id': target}))
            self.assertEqual(result['result'], {'state': state})

    def test_authorization_before_replay_and_immediately_before_acceptance(self):
        req = self.request()
        self.dispatch(req)
        for error in ('Expired', 'Revoked', 'Unauthorized', 'Unavailable', 'OutcomeUnknown'):
            self.error = error
            self.refuses(error, req)
        self.error = None
        calls = self.auth_calls
        def revoke_between_checks():
            return 'Revoked' if self.auth_calls > calls else None
        def authorize():
            value = revoke_between_checks()
            self.auth_calls += 1
            return value
        self.service.authorize = authorize
        self.refuses('Revoked', self.request())
        self.assertEqual(len(self.service.accepted), 1)

    def test_sender_identity_rechecked_before_acceptance(self):
        calls = self.auth_calls
        def authorize():
            self.auth_calls += 1
            if self.auth_calls == calls + 2:
                self.identity.closed = True
        self.service.authorize = authorize
        self.refuses('Unauthorized', self.request())
        self.assertFalse(self.service.accepted)

    def test_capacity_no_eviction_and_connection_bounds(self):
        # A bounded fixture pre-fills dedupe without thousands of pointless parses.
        self.service.accepted = {str(uuid.uuid4()): (b'hash', b'response') for _ in range(4096)}
        self.refuses('Busy', self.request())
        self.assertEqual(len(self.service.accepted), 4096)
        clients = [self.service.connect(os.getpid()) for _ in range(8)]
        try:
            with self.assertRaisesRegex(Refusal, 'Busy'):
                self.service.connect(os.getpid())
        finally:
            for client in clients:
                client.close()

    def test_kernel_credentials_write_is_actual_sender_not_socketpair_creator(self):
        client = self.service.connect(os.getpid())
        self.addCleanup(client.close)
        client.settimeout(2)
        os.write(client.fileno(), self.wire(self.request()))
        self.service.poll()
        self.assertEqual(json.loads(client.recv(65536))['outcome'], 'success')
        self.assertEqual(self.identity.calls[-1], self.credentials)
        # A separate process inherits the SAME client fd: SO_PEERCRED would
        # still name this parent. SCM_CREDENTIALS must name the actual child.
        packet = self.wire(self.request())
        code = 'import os,sys;os.write(int(sys.argv[1]),bytes.fromhex(sys.argv[2]))'
        child = subprocess.run([sys.executable, '-c', code, str(client.fileno()), packet.hex()],
                               pass_fds=(client.fileno(),), timeout=5)
        self.assertEqual(child.returncode, 0)
        self.service.poll()
        self.assertEqual(json.loads(client.recv(65536))['error'], 'Unauthorized')
        self.assertEqual(len(self.service.accepted), 1)

    def test_ancillary_rights_and_truncated_payload_refuse_without_fd_leak(self):
        client = self.service.connect(os.getpid())
        self.addCleanup(client.close)
        client.settimeout(2)
        with tempfile.TemporaryFile() as canary:
            before = len(os.listdir('/proc/self/fd'))
            client.sendmsg([self.wire(self.request())], [(socket.SOL_SOCKET, socket.SCM_RIGHTS,
                                                        array.array('i', [canary.fileno()]))])
            self.service.poll()
            self.assertEqual(json.loads(client.recv(65536))['error'], 'InvalidRequest')
            self.assertEqual(len(os.listdir('/proc/self/fd')), before)
        os.write(client.fileno(), b'x' * 65537)
        self.service.poll()
        self.assertEqual(json.loads(client.recv(65536))['error'], 'InvalidRequest')
        self.assertFalse(self.service.accepted)

    def test_channel_close_revocation_idle_deadline_and_fresh_epoch(self):
        client = self.service.connect(os.getpid())
        self.addCleanup(client.close)
        self.service.close()
        self.assertEqual(client.recv(1), b'')
        with self.assertRaisesRegex(Refusal, 'Unavailable'):
            self.service.connect(os.getpid())
        other = Service(dict(self.binding, epoch=str(uuid.uuid4())), Identity(), lambda: None)
        self.addCleanup(other.close)
        self.assertNotEqual(self.service.address, other.address)
        client2 = other.connect(os.getpid())
        self.addCleanup(client2.close)
        for fd, (channel, _) in list(other.channels.items()):
            other.channels[fd] = (channel, time.monotonic() - 1)
        other.poll()
        self.assertEqual(client2.recv(1), b'')

    def test_namespace_and_process_handles_are_pinned_and_dead_worker_refuses(self):
        identity = ns['MutationIdentity'](os.getpid())
        self.addCleanup(identity.close)
        self.assertEqual(len(identity.ns), 3)
        self.assertTrue(all(not os.get_inheritable(fd) for fd in identity.ns.values()))
        self.assertFalse(identity.check(os.getpid(), os.getuid() + 1, os.getgid()))
        self.assertFalse(identity.check(2**30, os.getuid(), os.getgid()))
        with patch.object(type(identity), 'process_start', return_value='changed'):
            self.assertFalse(identity.check(*self.credentials))
        child = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(5)'])
        try:
            dead = ns['MutationIdentity'](child.pid)
            self.addCleanup(dead.close)
            child.kill()
            child.wait(timeout=2)
            self.assertFalse(dead.check(*self.credentials))
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=2)

    def test_synthetic_endpoint_has_no_listener_and_cannot_be_replaced(self):
        self.assertFalse(os.path.exists(self.service.address))
        with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET) as client:
            with self.assertRaises(OSError):
                client.connect(self.service.address)
        host = (SOURCE / 'containment/contained_host.py').read_text()
        self.assertIn('path == mutation.address', host)
        self.assertIn("address[2:] in (path, path + b'\\0')", host)
        self.assertIn('original.family == socket.AF_UNIX', host)
        self.assertIn('== socket.SOCK_SEQPACKET', host)
        self.assertLess(host.index('path == mutation.address'), host.index('elif path.startswith(prefix)'))

    def test_live_broker_preflight_receipt_and_post_exec_probe(self):
        # Independent child watchdog: a broker regression cannot hang the suite.
        result = subprocess.run([sys.executable, '-m', 'tests.test_store_mutation', '--live-helper'],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30)
        if result.returncode == 77 and os.environ.get('AIDA_REQUIRE_STORE_MUTATION_LIVE') != '1':
            self.skipTest(result.stderr.strip())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('live-store-channel-ok', result.stdout)

    def test_live_required_launch_refuses_absence_stale_epoch_empty_grant_and_revocation(self):
        for fault in ('absent', 'wrong_epoch', 'empty_grant', 'revoked'):
            result = subprocess.run([sys.executable, '-m', 'tests.test_store_mutation', '--live-helper', fault],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30)
            if result.returncode == 77 and os.environ.get('AIDA_REQUIRE_STORE_MUTATION_LIVE') != '1':
                self.skipTest(result.stderr.strip())
            self.assertNotEqual(result.returncode, 0, fault + result.stdout + result.stderr)
            self.assertNotIn('live-store-channel-ok', result.stdout, fault)

    def broker_connect(self, path, kind=socket.SOCK_SEQPACKET, service=True, pid=None):
        # Exercise the actual broker branch with kernel notifications replaced
        # at the ioctl boundary. sockaddr is read from real process memory;
        # socket type, FD flags and injected socketpair are real kernel objects.
        import ctypes as C
        original = socket.socket(socket.AF_UNIX, kind)
        self.addCleanup(original.close)
        address = struct.pack('H', socket.AF_UNIX) + path
        memory = C.create_string_buffer(address)
        notification = ns['Notification']()
        notification.id = 41
        notification.pid = os.getpid() if pid is None else pid
        notification.data.arch = 0xc000003e
        notification.data.nr = 42
        notification.data.args[0] = original.fileno()
        notification.data.args[1] = C.addressof(memory)
        notification.data.args[2] = len(address)
        replies, injected = [], []
        def libc_ioctl(fd, operation, pointer):
            self.assertEqual(operation.value, ns['RECV'])
            C.memmove(pointer, C.byref(notification), C.sizeof(notification))
            return 0
        def ioctl(fd, operation, value):
            if operation == ns['ADDFD']:
                self.assertEqual(value.flags, 1)
                self.assertEqual(value.newfd, original.fileno())
                injected.append(socket.socket(fileno=os.dup(value.srcfd)))
                self.addCleanup(injected[-1].close)
            elif operation == ns['SEND']:
                replies.append(value.error)
            else:
                self.assertEqual(operation, ns['VALID'])
        with patch.object(ns['LIBC'], 'ioctl', libc_ioctl), patch.dict(ns,
                ioctl=ioctl, target_socket=lambda pid, fd: os.dup(fd)):
            ns['broker_connect_contained'](-1, -1, b'/private/state', None, 1, None, None,
                                          self.service if service else None)
        return replies, injected

    def test_broker_exact_epoch_seqpacket_only_no_host_path_fallback(self):
        replies, installed = self.broker_connect(self.service.address + b'\0')
        self.assertEqual(replies, [0])
        self.assertEqual(len(installed), 1)
        client = installed[0]
        client.settimeout(1)
        os.write(client.fileno(), self.wire(self.request()))
        self.service.poll()
        self.assertEqual(json.loads(client.recv(65536))['outcome'], 'success')
        for path in (b'\0' + self.service.address, self.service.address + b'/extra',
                     self.service.address + b'\0\0', self.service.address.replace(b'service', b'../service'),
                     ('/run/aida-mutation/%s/service' % uuid.uuid4()).encode()):
            replies, installed = self.broker_connect(path)
            self.assertNotEqual(replies, [0], path)
            self.assertFalse(installed)
        for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
            replies, installed = self.broker_connect(self.service.address, kind=kind)
            self.assertNotEqual(replies, [0])
            self.assertFalse(installed)
        replies, installed = self.broker_connect(self.service.address, service=False)
        self.assertNotEqual(replies, [0])
        self.assertFalse(installed)

    def test_queued_dead_sender_pidfd_refuses_even_if_numeric_pid_is_reused(self):
        child = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(5)'])
        try:
            sender_fd = os.pidfd_open(child.pid)
            self.addCleanup(os.close, sender_fd)
            child.kill()
            child.wait(timeout=2)
            identity = ns['MutationIdentity'](os.getpid())
            self.addCleanup(identity.close)
            # A new/current process with the claimed numeric PID cannot revive
            # the kernel-held sender object attached to the queued packet.
            self.assertFalse(identity.check(*self.credentials, sender_fd=sender_fd))
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=2)

    def test_client_retry_retains_request_identity(self):
        # Exercise the production client with a real local seqpacket server;
        # first delivery is deliberately lost, then the exact bytes replay.
        with tempfile.TemporaryDirectory() as tmp:
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
            path = tmp + '/service'
            listener.bind(path)
            listener.listen(2)
            listener.settimeout(3)
            seen = []
            errors = []
            def server():
                try:
                    for i in range(2):
                        conn, _ = listener.accept()
                        with conn:
                            packet = conn.recv(65536)
                            seen.append(packet)
                            if i == 1:
                                req = json.loads(packet)
                                conn.send(self.service.dispatch(packet, self.credentials))
                except BaseException as e:
                    errors.append(e)
            thread = threading.Thread(target=server)
            thread.start()
            real_socket = socket.socket
            class ClientSocket:
                def __init__(self, *args, **kwargs): self.sock = real_socket(*args, **kwargs)
                def __enter__(self): return self
                def __exit__(self, *args): self.sock.close()
                def __getattr__(self, name): return getattr(self.sock, name)
                def connect(self, endpoint): self.sock.connect(path)
            try:
                # Empty first response is uncertain delivery, not success.
                # The client currently treats malformed data as a bounded retry.
                with patch.dict(ns, socket=__import__('types').SimpleNamespace(
                        socket=ClientSocket, AF_UNIX=socket.AF_UNIX, SOCK_SEQPACKET=socket.SOCK_SEQPACKET,
                        SOCK_CLOEXEC=socket.SOCK_CLOEXEC, CMSG_SPACE=socket.CMSG_SPACE,
                        MSG_CMSG_CLOEXEC=socket.MSG_CMSG_CLOEXEC, SOL_SOCKET=socket.SOL_SOCKET,
                        SCM_RIGHTS=socket.SCM_RIGHTS, MSG_TRUNC=socket.MSG_TRUNC, MSG_CTRUNC=socket.MSG_CTRUNC)):
                    ns['mutation_probe'](self.service.address.decode())
            finally:
                thread.join(timeout=5)
                listener.close()
            self.assertFalse(thread.is_alive())
            self.assertFalse(errors, errors)
            self.assertEqual(len(seen), 2)
            self.assertEqual(seen[0], seen[1])
            self.assertEqual(len(self.service.accepted), 1)


def live_helper(fault='none'):
    """Synthetic shared-boundary fixture, never a real grant/vendor claim."""
    import select
    import shutil
    if not shutil.which('bwrap'):
        print('LIVE_UNAVAILABLE: bwrap missing', file=sys.stderr)
        return 77
    precheck = subprocess.run(['/usr/bin/bwrap', '--unshare-all', '--ro-bind', '/', '/',
                               '--proc', '/proc', '--dev', '/dev', '--', '/usr/bin/true'],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
    if precheck.returncode:
        print('LIVE_UNAVAILABLE: bwrap namespace prerequisite: ' + precheck.stderr.decode(), file=sys.stderr)
        return 77
    epoch = str(uuid.uuid4())
    endpoint = '/run/aida-mutation/%s/service' % epoch
    helper_auth, authority = socket.socketpair()
    receipt_r, receipt_w = os.pipe()
    acceptance_r, acceptance_w = os.pipe()
    fixture = tempfile.TemporaryDirectory()
    # /tmp is masked inside the lane; no fixture filesystem mount is needed.
    vendor = """import os,socket,json,uuid
endpoint=os.environ['AIDA_STORE_MUTATION_ENDPOINT']
s=socket.socket(socket.AF_UNIX,socket.SOCK_SEQPACKET)
s.settimeout(5)
s.connect(endpoint)
request=dict(version=1,launch_epoch=endpoint.split('/')[-2],request_id=str(uuid.uuid4()),op='session_probe',data=dict(challenge='after-exec'))
os.write(s.fileno(),json.dumps(request).encode())
reply=json.loads(s.recv(65536))
assert reply['result']['challenge']=='after-exec',reply
print('live-store-channel-ok',flush=True)
"""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as port_socket:
        port_socket.bind(('127.0.0.1', 0))
        port = port_socket.getsockname()[1]
    cfg = dict(grant='synthetic-fixture-only', ttl=30, port=port, allowed=[], state=fixture.name,
               policy_sha='fixture-policy', preflight=[], nested_userns=False,
               bwrap=['--unshare-all', '--die-with-parent', '--cap-drop', 'ALL', '--new-session',
                      '--ro-bind', '/', '/', '--dev', '/dev', '--proc', '/proc',
                      '--tmpfs', '/tmp', '--tmpfs', '/run'],
               argv=['/usr/bin/python3', '-I', '-S', '-c', vendor],
               mutation=dict(epoch=epoch, specs=[str(uuid.uuid4())], role='implementer',
                             authority_fd=helper_auth.fileno()))
    if fault == 'absent':
        cfg.pop('mutation')
    elif fault == 'wrong_epoch':
        cfg['mutation']['epoch'] = str(uuid.uuid4())
    elif fault == 'empty_grant':
        cfg['grant'] = ''
    lane_source = '\n'.join((SOURCE / name).read_text() for name in
                            ('containment/egress_probe.py', 'containment/contained_lane.py', 'store_mutation/client.py'))
    host_source = lane_source + '\n' + (SOURCE / 'store_mutation/service.py').read_text() + '\n' + (SOURCE / 'containment/contained_host.py').read_text()
    # Only the fixture bypasses keyring issuance. Production code has no such
    # selector, no test verb, and still uses the real isolated child keyring.
    host_source += "\nisolate_keyring = lambda grant, ttl: None\nSOURCE = sys.argv[1]\nsys.exit(host_contained(json.loads(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4])))"
    env = dict(os.environ, AIDA_STORE_MUTATION_ENDPOINT=endpoint)
    process = subprocess.Popen(['/usr/bin/python3', '-I', '-S', '-c', host_source, lane_source,
                                json.dumps(cfg), str(receipt_w), str(acceptance_r)],
                               env=env, pass_fds=(helper_auth.fileno(), receipt_w, acceptance_r))
    helper_auth.close()
    os.close(receipt_w)
    os.close(acceptance_r)
    receipt = b''
    end = time.monotonic() + 20
    accepted = False
    try:
        while process.poll() is None:
            if time.monotonic() >= end:
                raise RuntimeError('live channel fixture deadline')
            ready, _, _ = select.select([authority] + ([] if accepted else [receipt_r]), [], [], .05)
            if authority in ready:
                if authority.recv(1) != b'?':
                    break
                authority.sendall(b'R' if fault == 'revoked' and accepted else b'Y')
            if receipt_r in ready:
                chunk = os.read(receipt_r, 8192)
                if not chunk:
                    break
                receipt += chunk
                if receipt.endswith(b'\n'):
                    obj = json.loads(receipt)
                    assert obj['mutation_epoch'] == epoch and obj['mutation_ready'], obj
                    os.write(acceptance_w, b'R')
                    accepted = True
        return process.wait(timeout=3)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        authority.close()
        for fd in (receipt_r, acceptance_w):
            os.close(fd)
        fixture.cleanup()


if __name__ == '__main__':
    if len(sys.argv) in (2, 3) and sys.argv[1] == '--live-helper':
        fault = sys.argv[2] if len(sys.argv) == 3 else 'none'
        assert fault in ('none', 'absent', 'wrong_epoch', 'empty_grant', 'revoked')
        sys.exit(live_helper(fault))
    unittest.main()
