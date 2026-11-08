"""Typed v1 broker. Canonical writes run only in the trusted Rust host.
trace:TASK-3225 | ai:codex
"""
import hashlib
import uuid

# Linux UAPI: a kernel-held sender pidfd closes the queued-packet PID reuse race.
SO_PASSPIDFD, SCM_PIDFD = 76, 4
MUTATION_LIMIT = 65536
# Canonical schemas are staged until Git/oplog publication is transactional.
MUTATION_VERBS = ('session_probe', 'request_status')
CANONICAL_VERBS = ('comment_add', 'spec_edit')
# trace:TASK-3223 | ai:codex
MUTATION_ERRORS = frozenset(('InvalidRequest', 'UnsupportedVersion', 'UnsupportedOperation',
                            'Unauthorized', 'Expired', 'Revoked', 'Conflict', 'Busy',
                            'Unavailable', 'OutcomeUnknown'))


class MutationRefusal(Exception):
    pass


def mutation_uuid(value):
    if not isinstance(value, str) or str(uuid.UUID(value)) != value:
        raise ValueError('invalid UUID')
    return value


def mutation_request(raw):
    if len(raw) > MUTATION_LIMIT:
        raise MutationRefusal('InvalidRequest')
    def pairs(rows):
        if len(rows) > 32 or len(dict(rows)) != len(rows):
            raise ValueError('object bounds')
        return dict(rows)
    def bounded(value, depth=0):
        if depth > 16:
            raise ValueError('depth')
        if isinstance(value, str):
            if len(value.encode('utf-8')) > 16384 or any((ord(c) < 32 or 127 <= ord(c) <= 159) and c not in '\n\t' for c in value):
                raise ValueError('string bounds')
        elif isinstance(value, dict):
            for k, v in value.items():
                bounded(k, depth + 1)
                bounded(v, depth + 1)
        elif isinstance(value, list):
            for v in value:
                bounded(v, depth + 1)
    try:
        obj = json.loads(raw.decode('utf-8'), object_pairs_hook=pairs,
                         parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
        bounded(obj)
        if not isinstance(obj, dict) or set(obj) != {'version', 'launch_epoch', 'request_id', 'op', 'data'}:
            raise ValueError('envelope')
        if type(obj['version']) is not int:
            raise ValueError('version')
        mutation_uuid(obj['launch_epoch'])
        mutation_uuid(obj['request_id'])
        if not isinstance(obj['op'], str) or not isinstance(obj['data'], dict):
            raise ValueError('operation')
        # Closed schemas also exclude paths, authority selectors and replacement objects.
        if obj['version'] != 1:
            raise MutationRefusal('UnsupportedVersion')
        if obj['op'] not in MUTATION_VERBS + CANONICAL_VERBS:
            raise MutationRefusal('UnsupportedOperation')
        if obj['op'] == 'session_probe':
            if set(obj['data']) != {'challenge'} or not isinstance(obj['data']['challenge'], str) or len(obj['data']['challenge'].encode()) > 128:
                raise ValueError('challenge')
        elif obj['op'] == 'request_status':
            if set(obj['data']) != {'request_id'}:
                raise ValueError('status')
            mutation_uuid(obj['data']['request_id'])
        else:
            data = obj['data']
            keys = {'spec_id', 'expected_revision'}
            if obj['op'] == 'comment_add':
                keys.add('content')
            if obj['op'] == 'spec_edit':
                keys.add('fields')
            if set(data) != keys or not isinstance(data['spec_id'], str) or not 1 <= len(data['spec_id']) <= 128:
                raise ValueError('target')
            revision = data['expected_revision']
            if not isinstance(revision, str) or len(revision) != 64 or any(c not in '0123456789abcdef' for c in revision):
                raise ValueError('revision')
            if obj['op'] == 'comment_add':
                if not isinstance(data['content'], str) or not data['content']:
                    raise ValueError('comment')
            elif obj['op'] == 'spec_edit':
                fields = data['fields']
                if not isinstance(fields, dict) or not fields or not set(fields) <= {'title', 'description'}:
                    raise ValueError('fields')
                if any(not isinstance(v, str) for v in fields.values()):
                    raise ValueError('field value')
                if 'title' in fields and (not fields['title'] or len(fields['title'].encode()) > 512):
                    raise ValueError('title')
        return obj
    except (ValueError, TypeError, RecursionError, OverflowError):
        raise MutationRefusal('InvalidRequest') from None


class MutationAuthority:
    """A timeout permanently shuts down all copies of the authority endpoint.
    The Rust receipt-barrier owner shares this socket; close alone would leave
    its duplicate able to consume an uncorrelated, late preflight reply.
    """
    def __init__(self, channel):
        self.channel = channel
        self.failed = False
        # The approved admission/I/O bound also allows transient host disk stalls.
        # trace:TASK-3225 | ai:codex
        channel.settimeout(5)

    def __call__(self):
        if self.failed:
            return 'Unavailable'
        try:
            self.channel.sendall(b'?')
            reply = self.channel.recv(1)
            if reply in (b'Y', b'E', b'R', b'N'):
                return {b'Y': None, b'E': 'Expired', b'R': 'Revoked', b'N': 'Unauthorized'}[reply]
        except OSError:
            pass
        self.failed = True
        try:
            self.channel.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.channel.close()
        return 'Unavailable'


    # Framed, correlated host RPC. A lost reply has unknown outcome: never
    # reconnect or fall back to direct persistence. trace:TASK-3223 | ai:codex
    def operation(self, request):
        if self.failed:
            raise MutationRefusal('Unavailable')
        wire = json.dumps(request, separators=(',', ':')).encode()
        def receive(size):
            value = b''
            while len(value) < size:
                chunk = self.channel.recv(size - len(value))
                if not chunk:
                    raise OSError('host closed')
                value += chunk
            return value
        try:
            self.channel.sendall(b'M' + struct.pack('!I', len(wire)) + wire)
            size = struct.unpack('!I', receive(4))[0]
            if not 0 < size <= MUTATION_LIMIT:
                raise ValueError('host frame')
            response = json.loads(receive(size))
            if response['request_id'] != request['request_id'] or response['launch_epoch'] != request['launch_epoch']:
                raise ValueError('host correlation')
        except (OSError, ValueError, KeyError):
            self.failed = True
            try:
                self.channel.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.channel.close()
            raise MutationRefusal('OutcomeUnknown') from None
        if response.get('error'):
            raise MutationRefusal(response['error'] if response['error'] in MUTATION_ERRORS else 'OutcomeUnknown')
        return response['result']


class MutationIdentity:
    """Pin namespace handles and a pidfd, not reusable PID/path strings."""
    def __init__(self, worker):
        self.worker = worker
        self.pidfd = os.pidfd_open(worker)
        self.ns = {}
        try:
            for name in ('pid', 'user', 'mnt'):
                self.ns[name] = os.open('/proc/%d/ns/%s' % (worker, name), os.O_RDONLY | os.O_CLOEXEC)
            self.start = self.process_start(worker)
        except BaseException:
            self.close()
            raise

    @staticmethod
    def process_start(pid):
        with open('/proc/%d/stat' % pid) as f:
            return f.read().rsplit(')', 1)[1].split()[19]

    def check(self, pid, uid, gid, sender_fd=None):
        try:
            poll = select.poll()
            poll.register(self.pidfd, select.POLLIN)
            if poll.poll(0) or uid != os.getuid() or gid != os.getgid() or pid <= 0:
                return False
            if sender_fd is not None:
                poll.register(sender_fd, select.POLLIN)
                if poll.poll(0):
                    return False
                with open('/proc/self/fdinfo/%d' % sender_fd) as info:
                    actual = next(int(line.split()[1]) for line in info if line.startswith('Pid:'))
                if actual != pid:
                    return False
            before = self.process_start(pid)
            if self.process_start(self.worker) != self.start:
                return False
            for name, fd in self.ns.items():
                md = os.stat('/proc/%d/ns/%s' % (pid, name))
                if (md.st_dev, md.st_ino) != ns_identity(fd):
                    return False
            with open('/proc/%d/uid_map' % pid) as f:
                if [line.split() for line in f] != [[str(os.getuid()), str(os.getuid()), '1']]:
                    return False
            return before == self.process_start(pid) and not poll.poll(0)
        except (OSError, ValueError, IndexError, StopIteration):
            return False

    def close(self):
        for fd in list(self.ns.values()) + [self.pidfd]:
            os.close(fd)
        self.ns.clear()


class StoreMutationSession:
    """Single event-loop owner: at most one dispatch and eight connections.
    Authorization runs before replay as well as immediately before acceptance.
    Read-only dedupe is local; mutation decisions and retries belong to Rust.
    """
    def __init__(self, binding, identity, authorize):
        self.epoch = mutation_uuid(binding['epoch'])
        self.address = ('/run/aida-mutation/%s/service' % self.epoch).encode()
        self.binding = binding
        self.identity = identity
        self.authorize = authorize
        self.channels = {}
        self.accepted = {}
        self.closed = False
        self.probed = False

    def connect(self, pid, uid=None, gid=None):
        if self.closed:
            raise MutationRefusal('Unavailable')
        self.live()
        if not self.identity.check(pid, os.getuid() if uid is None else uid,
                                   os.getgid() if gid is None else gid):
            raise MutationRefusal('Unauthorized')
        if len(self.channels) >= 8:
            raise MutationRefusal('Busy')
        server, client = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET | socket.SOCK_CLOEXEC)
        try:
            server.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
            server.setsockopt(socket.SOL_SOCKET, SO_PASSPIDFD, 1)
        except BaseException:
            server.close()
            client.close()
            raise
        server.setblocking(False)
        self.channels[server.fileno()] = (server, time.monotonic() + 5)
        return client

    def live(self):
        error = self.authorize()
        if error:
            raise MutationRefusal(error if error in MUTATION_ERRORS else 'Unauthorized')

    def dispatch(self, raw, credentials, sender_fd=None):
        self.live()
        if self.closed:
            raise MutationRefusal('Unavailable')
        if not self.identity.check(*credentials, sender_fd=sender_fd):
            raise MutationRefusal('Unauthorized')
        request = mutation_request(raw)
        if request['op'] not in MUTATION_VERBS:
            raise MutationRefusal('UnsupportedOperation')
        if request['launch_epoch'] != self.epoch:
            raise MutationRefusal('Unauthorized')
        key = request['request_id']
        digest = hashlib.sha256(json.dumps([request['op'], request['data']],
                                          sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()).digest()
        previous = self.accepted.get(key)
        self.live()
        if not self.identity.check(*credentials, sender_fd=sender_fd):
            raise MutationRefusal('Unauthorized')
        if previous and digest != previous[0]:
            raise MutationRefusal('Conflict')
        if previous and request['op'] in ('session_probe', 'request_status'):
            return previous[1]
        if previous is None and len(self.accepted) >= 4096:
            raise MutationRefusal('Busy')
        if request['op'] == 'session_probe':
            result = dict(challenge=request['data']['challenge'], specs=self.binding['specs'],
                          role=self.binding['role'], permitted_verbs=list(MUTATION_VERBS))
            self.probed = True
        elif request['op'] == 'request_status':
            if request['data']['request_id'] in self.accepted:
                result = dict(state='completed')
            elif hasattr(self.authorize, 'operation'):
                result = self.authorize.operation(request)
            else:
                result = dict(state='unknown')
        else:
            if not hasattr(self.authorize, 'operation'):
                raise MutationRefusal('Unavailable')
            result = self.authorize.operation(request)
        response = dict(version=1, launch_epoch=self.epoch, request_id=key, outcome='success', result=result)
        wire = json.dumps(response, separators=(',', ':')).encode()
        if len(wire) > MUTATION_LIMIT:
            raise MutationRefusal('Unavailable')
        self.accepted[key] = (digest, wire)
        return wire

    def poll(self):
        for fd, (channel, deadline) in list(self.channels.items()):
            try:
                raw, ancillary, flags, _ = channel.recvmsg(MUTATION_LIMIT, socket.CMSG_SPACE(12) + socket.CMSG_SPACE(256), socket.MSG_CMSG_CLOEXEC)
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    channel.close()
                    del self.channels[fd]
                continue
            sender_fds = []
            try:
                credentials = []
                invalid = bool(flags & (socket.MSG_TRUNC | socket.MSG_CTRUNC))
                for level, kind, value in ancillary:
                    if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                        for received in array.array('i', value):
                            os.close(received)
                        invalid = True
                    elif level == socket.SOL_SOCKET and kind == SCM_PIDFD and len(value) == 4:
                        sender_fds.append(struct.unpack('i', value)[0])
                    elif level == socket.SOL_SOCKET and kind == socket.SCM_CREDENTIALS and len(value) == 12:
                        credentials.append(struct.unpack('3i', value))
                    else:
                        invalid = True
                if not raw and not ancillary:
                    raise EOFError()
                if invalid or len(credentials) != 1 or len(sender_fds) != 1:
                    raise MutationRefusal('InvalidRequest')
                response = self.dispatch(raw, credentials[0], sender_fds[0])
            except MutationRefusal as error:
                # Refusals never echo unvalidated input or host diagnostics.
                response = json.dumps(dict(version=1, launch_epoch=self.epoch, request_id=None,
                                           outcome='error', error=str(error))).encode()
            except EOFError:
                response = None
            finally:
                for fd_to_close in sender_fds:
                    os.close(fd_to_close)
            try:
                if response is not None:
                    channel.send(response, socket.MSG_DONTWAIT | socket.MSG_NOSIGNAL)
                    self.channels[fd] = (channel, time.monotonic() + 5)
                else:
                    raise OSError()
            except OSError:
                channel.close()
                del self.channels[fd]

    def close(self):
        self.closed = True
        for channel, _ in self.channels.values():
            channel.close()
        self.channels.clear()
        self.accepted.clear()
        self.identity.close()
