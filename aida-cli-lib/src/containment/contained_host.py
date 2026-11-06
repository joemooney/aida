
# ---- contained_host.py: host supervisor helper for a contained launch -----
# Appended to egress_probe.py (typed channels, proxy destination policy,
# seccomp notification ABI) and run by Rust as fixed code. Owns: the lane's
# isolated session keyring, the bwrap process, the connect broker and the
# allow-listed CONNECT proxy. Holds no model credential of its own; the API
# key reaches the worker only as its own env var in the exact environment
# Rust built. trace:TASK-2707 | ai:claude
ECONNREFUSED_ERR = -errno.ECONNREFUSED
KEYCTL_JOIN, KEYCTL_SETPERM, KEYCTL_SEARCH, KEYCTL_READ, KEYCTL_TIMEOUT = 1, 5, 10, 11, 15
SESSION, POSSESSOR_ONLY = -3, 0x3f000000


def keyctl(*args):
    return LIBC.syscall(250, *[C.c_long(a) if isinstance(a, int) else a for a in args])


def isolate_keyring(grant, ttl):
    """Give the lane a NEW anonymous session keyring holding only the child
    grant's key (or nothing, for a baseline child). The previous session
    keyring, which may link the launcher's own grant, is not linked in."""
    payload = None
    if grant:
        desc = ('aida-grant:' + grant).encode()
        key = keyctl(KEYCTL_SEARCH, SESSION, C.c_char_p(b'user'), C.c_char_p(desc))
        require(key >= 0, 'child grant key is not possessed by the launcher')
        buf = C.create_string_buffer(256)
        n = keyctl(KEYCTL_READ, key, buf, 256)
        require(0 <= n <= 256, 'child grant key unreadable')
        payload = buf.raw[:n]
    ring = keyctl(KEYCTL_JOIN, C.c_char_p(None))
    require(ring >= 0, 'cannot create the lane session keyring')
    require(keyctl(KEYCTL_SETPERM, ring, POSSESSOR_ONLY) >= 0, 'cannot restrict lane keyring')
    if payload is not None:
        key = LIBC.syscall(248, C.c_char_p(b'user'), C.c_char_p(desc), C.c_char_p(payload),
                           C.c_size_t(len(payload)), C.c_long(SESSION))
        require(key >= 0, 'cannot install the child grant key')
        require(keyctl(KEYCTL_SETPERM, key, POSSESSOR_ONLY) >= 0 and
                keyctl(KEYCTL_TIMEOUT, key, max(1, int(ttl))) >= 0,
                'cannot restrict the child grant key')


def openat2_beneath(dirfd, relative):
    class How(C.Structure):
        _fields_ = [('flags', C.c_ulonglong), ('mode', C.c_ulonglong),
                    ('resolve', C.c_ulonglong)]
    # RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH
    how = How(os.O_PATH | os.O_CLOEXEC, 0, 0x02 | 0x04 | 0x08)
    fd = LIBC.syscall(437, dirfd, C.c_char_p(relative), C.byref(how), C.sizeof(how))
    if fd < 0:
        raise OSError(C.get_errno(), 'private endpoint unresolvable')
    return fd


def fd_flags(pid, fd):
    with open('/proc/%d/fdinfo/%d' % (pid, fd)) as f:
        for line in f:
            if line.startswith('flags:'):
                return int(line.split()[1], 8)
    raise RuntimeError('target descriptor flags unavailable')


SIOCGSKNS = 0x894C


def target_socket(pid, target_fd):
    """A host-side duplicate of the trapped task's descriptor (pidfd_getfd):
    the very socket object the task named, not a path or caller claim."""
    pidfd = os.pidfd_open(pid)
    try:
        fd = LIBC.syscall(438, pidfd, target_fd, 0)
        if fd < 0:
            raise OSError(C.get_errno(), 'target socket unavailable')
    finally:
        os.close(pidfd)
    os.set_inheritable(fd, False)
    return fd


# Kernel namespace ancestry, never a caller's namespace claim.
# trace:TASK-2708 | ai:codex
NS_GET_USERNS, NS_GET_PARENT = 0xb701, 0xb702
CONNECT_SLOTS = threading.BoundedSemaphore(16)
CONNECT_TIMEOUT = 3


def ns_identity(fd):
    md = os.fstat(fd)
    return (md.st_dev, md.st_ino)


def socket_scope(fd, lane_net, lane_user):
    ns = checked(LIBC.ioctl(fd, C.c_ulong(SIOCGSKNS), 0), 'SOCKET_NETNS_UNREADABLE')
    owner = None
    try:
        if ns_identity(ns) == lane_net:
            return 'lane'
        owner = checked(LIBC.ioctl(ns, NS_GET_USERNS, 0), 'SOCKET_USERNS_UNREADABLE')
        # Linux bounds user-namespace nesting at 32; retain a separate bound.
        for _ in range(64):
            if ns_identity(owner) == lane_user:
                return 'nested'
            parent = checked(LIBC.ioctl(owner, NS_GET_PARENT, 0), 'SOCKET_USERNS_NOT_DESCENDANT')
            os.close(owner)
            owner = parent
        return 'foreign'
    finally:
        os.close(ns)
        if owner is not None:
            os.close(owner)


def sock_type(fd):
    value, size = C.c_int(0), C.c_uint(4)
    if LIBC.getsockopt(fd, socket.SOL_SOCKET, socket.SO_TYPE, C.byref(value), C.byref(size)) < 0:
        raise OSError(C.get_errno(), 'target socket type unavailable')
    return value.value


NESTED_CONNECT_PROGRAM = r"""
import ctypes as C,errno,os,sys
lib=C.CDLL(None,use_errno=True)
if lib.prctl(1,9,0,0,0)!=0 or os.getppid()!=int(sys.argv[3]):
    os._exit(errno.EIO)
address=bytes.fromhex(sys.argv[2])
rc=lib.connect(int(sys.argv[1]),C.c_char_p(address),C.c_uint(len(address)))
os._exit(0 if rc==0 else min(C.get_errno(),255))
"""


def connect_nested(listener, ident, dup, address):
    """Bound each pinned connect with a disposable process and a deadline.
    Socket flags/timeouts are shared with the lane: a sibling could clear
    SO_SNDTIMEO. A fresh interpreter avoids Python-after-threaded-fork work.
    Never CONTINUE, and never change the shared O_NONBLOCK flags.
    trace:TASK-2708 | ai:codex"""
    child = None
    try:
        child = subprocess.Popen(['/usr/bin/python3', '-I', '-S', '-c',
                                  NESTED_CONNECT_PROGRAM, str(dup), address.hex(), str(os.getpid())],
                                 pass_fds=(dup,), env={}, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            code = child.wait(timeout=CONNECT_TIMEOUT)
            error = -code if code >= 0 else -errno.EIO
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
            error = -errno.ETIMEDOUT
        ioctl(listener, SEND, Response(ident, 0, error, 0))
    except (OSError, RuntimeError):
        try:
            ioctl(listener, SEND, Response(ident, 0, -errno.EIO, 0))
        except RuntimeError:
            pass
    finally:
        if child is not None and child.poll() is None:
            child.kill()
            child.wait()
        os.close(dup)
        CONNECT_SLOTS.release()


# Snapshot send arguments and execute on the duplicated socket. Inspecting a
# mutable msghdr then CONTINUE would allow a sibling to swap msg_name.
# trace:TASK-2708 | ai:codex
class SendIov(C.Structure):
    _fields_ = [('base', C.c_void_p), ('size', C.c_size_t)]


class SendHeader(C.Structure):
    _fields_ = [('name', C.c_void_p), ('namelen', C.c_uint),
                ('iov', C.c_void_p), ('iovlen', C.c_size_t),
                ('control', C.c_void_p), ('controllen', C.c_size_t), ('flags', C.c_int)]


def broker_send(listener, n):
    dup, mem, rights = None, None, []
    try:
        dup = target_socket(n.pid, n.data.args[0])
        mem = os.open('/proc/%d/mem' % n.pid, os.O_RDONLY | os.O_CLOEXEC)
        def read(ptr, size):
            require(size <= 1048576, 'send exceeds bound')
            data = os.pread(mem, size, ptr) if size else b''
            require(len(data) == size, 'short send memory')
            return data
        ioctl(listener, VALID, C.c_ulonglong(n.id))
        if n.data.nr == 44:
            _, buf, size, flags, name, namelen = n.data.args
            buffers, ancillary = [read(buf, size)], []
        else:
            flags = n.data.args[2]
            h = SendHeader.from_buffer_copy(read(n.data.args[1], C.sizeof(SendHeader)))
            name, namelen = h.name, h.namelen
            require(h.iovlen <= 64 and h.controllen <= 4096, 'send header exceeds bound')
            rows = read(h.iov, h.iovlen * C.sizeof(SendIov))
            iovs = (SendIov * h.iovlen).from_buffer_copy(rows)
            require(sum(v.size for v in iovs) <= 1048576, 'send payload exceeds bound')
            buffers = [read(v.base, v.size) for v in iovs]
            control, ancillary = read(h.control, h.controllen), []
            offset = 0
            while offset < len(control):
                require(len(control) - offset >= 16, 'short send control')
                size, level, kind = struct.unpack_from('Qii', control, offset)
                require(16 <= size <= len(control) - offset and
                        level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS,
                        'send control refused')
                data = control[offset + 16:offset + size]
                require(len(data) % 4 == 0, 'invalid rights')
                copied = array.array('i')
                for fd in array.array('i', data):
                    copied.append(target_socket(n.pid, fd))
                    rights.append(copied[-1])
                ancillary.append((level, kind, copied))
                offset += (size + 7) & ~7
        require(not flags & 0x20000000, 'FASTOPEN refused')
        require(namelen <= 128, 'send address exceeds bound')
        with socket.socket(fileno=dup) as sock:
            dup = None
            require(not (sock.family == socket.AF_UNIX and name), 'named Unix send refused')
            # Named sendmsg is conservatively refused; connected sendmsg and
            # SCM_RIGHTS remain available for anonymous sandbox IPC.
            require(n.data.nr == 44 or not name, 'named sendmsg refused')
            address = read(name, namelen) if name else b''
            ioctl(listener, VALID, C.c_ulonglong(n.id))
            if n.data.nr == 44:
                rc = LIBC.sendto(sock.fileno(), C.c_char_p(buffers[0]), C.c_size_t(len(buffers[0])),
                                 C.c_int(flags | socket.MSG_DONTWAIT),
                                 C.c_char_p(address) if name else None, C.c_uint(len(address)))
                if rc < 0:
                    raise OSError(C.get_errno(), 'sendto refused')
            else:
                rc = sock.sendmsg(buffers, ancillary, flags | socket.MSG_DONTWAIT)
        ioctl(listener, SEND, Response(n.id, rc, 0, 0))
    except (OSError, RuntimeError, ValueError) as error:
        code = error.errno if isinstance(error, OSError) else errno.EACCES
        try:
            ioctl(listener, SEND, Response(n.id, 0, -code, 0))
        except RuntimeError:
            pass
    finally:
        for fd in rights + ([dup] if dup is not None else []) + ([mem] if mem is not None else []):
            os.close(fd)


def broker_connect_contained(listener, state_fd, state_prefix, connector, port, lane_net, lane_user):
    """F-a broker: read the sockaddr ONCE, never CONTINUE. A lane-netns
    stream socket naming the relay port on 127.0.0.1 gets a relay-connected
    socket; a pathname beneath the private state root gets the pinned private
    endpoint; an inet socket the kernel places in a nested netns (never the
    lane's) is connected as-is by the broker; everything else refuses.
    trace:TASK-2708 | ai:claude"""
    n = Notification()
    if LIBC.ioctl(listener, C.c_ulonglong(RECV), C.byref(n)) < 0:
        # ENOENT: the trapped task was interrupted or every filtered task is
        # gone (the listener reports hang-up). Nothing to answer.
        require(C.get_errno() in (errno.ENOENT, errno.EINTR),
                'seccomp notification facility unavailable')
        return False
    require(n.data.arch == 0xc000003e and n.data.nr in (42, 44, 46) and n.flags == 0,
            'unexpected socket broker notification')
    if n.data.nr in (44, 46):
        broker_send(listener, n)
        return True
    ident = C.c_ulonglong(n.id)
    answer = -errno.EACCES
    try:
        length, target_fd = n.data.args[2], n.data.args[0]
        require(2 <= length <= 128 and target_fd < 2**31, 'socket address exceeds bound')
        mem = os.open('/proc/%d/mem' % n.pid, os.O_RDONLY | os.O_CLOEXEC)
        try:
            ioctl(listener, VALID, ident)
            address = os.pread(mem, length, n.data.args[1])
        finally:
            os.close(mem)
        ioctl(listener, VALID, ident)
        require(len(address) == length, 'short socket address')
        family = struct.unpack('H', address[:2])[0]
        connected = None
        if family in (socket.AF_INET, socket.AF_INET6):
            answer = ECONNREFUSED_ERR
            # trace:TASK-2708 | ai:claude — the target's own socket decides:
            # its netns (nested or lane) and its type. Any failure refuses.
            try:
                dup = target_socket(n.pid, target_fd)
            except OSError:
                dup = None  # bad fd, not a socket, gone task: refuse
            kind = nested = None
            if dup is not None:
                try:
                    ioctl(listener, VALID, ident)
                    kind = sock_type(dup)
                    scope = socket_scope(dup, lane_net, lane_user)
                    nested = scope == 'nested' if scope != 'foreign' else None
                except (OSError, RuntimeError):
                    kind = nested = None
                except BaseException:
                    os.close(dup)
                    raise
                if nested:
                    if CONNECT_SLOTS.acquire(blocking=False):
                        try:
                            threading.Thread(target=connect_nested,
                                             args=(listener, n.id, dup, address), daemon=True).start()
                        except BaseException:
                            CONNECT_SLOTS.release()
                            os.close(dup)
                            raise
                        return True
                    answer = -errno.EAGAIN
                os.close(dup)
            if family == socket.AF_INET and kind == socket.SOCK_STREAM and nested is False:
                require(length >= 8, 'short inet address')
                dport = struct.unpack('!H', address[2:4])[0]
                if address[4:8] == b'\x7f\x00\x00\x01' and dport == port:
                    connector.send(b'connect-v1')
                    reply, fds = receive(connector)
                    require(reply == b'connected-v1' and len(fds) == 1,
                            'relay connector refused')
                    connected = socket.socket(fileno=fds[0])
        elif family == socket.AF_UNIX:
            path = address[2:].rstrip(b'\0')
            prefix = state_prefix + b'/'
            if path.startswith(prefix) and b'\0' not in path and len(path) > len(prefix):
                target = openat2_beneath(state_fd, path[len(prefix):])
                try:
                    md = os.fstat(target)
                    require(stat.S_ISSOCK(md.st_mode) and md.st_uid == os.getuid()
                            and md.st_nlink == 1, 'private endpoint identity refused')
                    connected = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    connected.settimeout(1)
                    connected.connect('/proc/self/fd/%d' % target)
                finally:
                    os.close(target)
        if connected is not None:
            try:
                flags = fd_flags(n.pid, target_fd)
                ioctl(listener, VALID, ident)
                connected.setblocking(not (flags & os.O_NONBLOCK))
                ioctl(listener, ADDFD, Addfd(n.id, 1, connected.fileno(), target_fd,
                                             os.O_CLOEXEC if flags & 0o2000000 else 0))
                ioctl(listener, SEND, Response(n.id, 0, 0, 0))
                return True
            finally:
                connected.close()
    except RuntimeError as error:
        if str(error).startswith('seccomp notification facility unavailable'):
            raise
    except OSError as error:
        if error.errno not in (errno.ENOENT, errno.ELOOP, errno.ECONNREFUSED, errno.EXDEV,
                               errno.ENOTDIR, errno.ESRCH):
            raise RuntimeError('socket broker facility refused') from error
    try:
        ioctl(listener, SEND, Response(n.id, 0, answer, 0))
    except RuntimeError:
        pass  # the trapped task died or was cancelled; nothing was installed
    return True


def _hung_up(fd):
    p = select.poll()
    p.register(fd, select.POLLIN)
    return any(ev & (select.POLLHUP | select.POLLERR) for _, ev in p.poll(0))


def proxy_request(data, fds, allowed, fixture):
    """Validate one typed CONNECT, pin a checked public address, connect on
    the host and hand the connected socket back over the per-request pair."""
    reply_fd = fds[0] if len(fds) == 1 else None
    for fd in fds[1:]:
        os.close(fd)
    if reply_fd is None:
        return
    with socket.socket(fileno=reply_fd) as reply:
        try:
            def no_duplicates(pairs):
                d = dict(pairs)
                require(len(d) == len(pairs), 'duplicate proxy field')
                return d
            request = json.loads(data, object_pairs_hook=no_duplicates)
            def resolve(name, port):
                if fixture and name == fixture[0] and port == fixture[1]:
                    return [(socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP, '',
                             (fixture[2], port))]
                return socket.getaddrinfo(name, port, 0, socket.SOCK_STREAM,
                                          socket.IPPROTO_TCP)[:16]
            family, kind, proto, address = destination(request, allowed, resolve, fixture)
            with socket.socket(family, kind, proto) as tunnel:
                tunnel.settimeout(10)
                tunnel.connect(address)
                tunnel.settimeout(None)
                send_fd(reply, b'tunnel-v1', tunnel.fileno())
            # Bounded metadata only: host, port, decision. Never payloads.
            print('aida-containment: proxy allowed %s:%d' % (request['host'], request['port']),
                  file=sys.stderr, flush=True)
        except (RuntimeError, ValueError, TypeError, KeyError, OSError):
            try:
                reply.send(b'denied-v1')
            except OSError:
                pass


def host_contained(cfg, receipt_fd, acceptance_fd):
    require(platform.machine() == 'x86_64', 'socket broker architecture unsupported')
    abi = LIBC.syscall(444, 0, 0, 1)
    require(abi > 0, 'Landlock capability unavailable; socket-policy selection refused')
    require(abi < 9, 'Landlock Unix-connect backend required on ABI>=9; broker not selected')
    sizes = (C.c_ushort * 3)()
    checked(LIBC.syscall(317, 3, 0, C.byref(sizes)), 'seccomp notification size probe unavailable')
    require(list(sizes) == [C.sizeof(Notification), C.sizeof(Response), C.sizeof(Data)],
            'seccomp notification ABI unsupported')
    isolate_keyring(cfg['grant'], cfg['ttl'])
    broker_mode = True
    port = cfg['port']
    allowed = {(h, 443) for h in cfg['allowed']}
    fixture = tuple(cfg['fixture']) if cfg.get('fixture') else None
    if fixture:
        allowed.add((fixture[0], fixture[1]))
    state_prefix = os.fsencode(cfg['state'])
    state_fd = os.open(cfg['state'], os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    broker, broker_child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    proxy, proxy_child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    connector, connector_child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    info_r, info_w = os.pipe()
    block_r, block_w = os.pipe()
    args = cfg['bwrap']
    fds = {info_w, block_r, proxy_child.fileno(), connector_child.fileno()}
    if broker_mode:
        fds.add(broker_child.fileno())
    for i, value in enumerate(args):
        if value in ('--bind-fd', '--ro-bind-fd'):
            fds.add(int(args[i + 1]))
    command = ['/usr/bin/bwrap', '--info-fd', str(info_w), '--block-fd', str(block_r)] + args + [
        '--', '/usr/bin/python3', '-I', '-S', '-c',
        SOURCE + '\ncontained_main(*lane_args(sys.argv))',
        str(broker_child.fileno() if broker_mode else -1), str(proxy_child.fileno()),
        str(connector_child.fileno()), str(port), json.dumps(cfg['preflight']), json.dumps(cfg.get('nested_userns', False))] + cfg['argv']
    # The exact allow-listed environment Rust built for this helper, unchanged.
    process = subprocess.Popen(command, pass_fds=tuple(fds), env=dict(os.environ))
    for fd in (info_w, block_r):
        os.close(fd)
    for s in (broker_child, proxy_child, connector_child):
        s.close()
    notification = None
    try:
        raw, end = b'', time.monotonic() + 20
        while True:
            require(time.monotonic() < end, 'bwrap readiness deadline')
            ready, _, _ = select.select([info_r], [], [], 0.1)
            if ready:
                chunk = os.read(info_r, 4096)
                require(chunk, 'bwrap exited before namespace readiness')
                raw += chunk
                try:
                    info = json.loads(raw)
                    break
                except ValueError:
                    continue
            require(process.poll() is None, 'bwrap exited before namespace readiness')
        worker = int(info['child-pid'])
        # Identity is judged HOST-side: an in-namespace self read cannot.
        identity = [[str(os.getuid()), str(os.getuid()), '1']]
        mapping, end = None, time.monotonic() + 5
        while mapping != identity and time.monotonic() < end:
            # bwrap's child writes its own map after readiness; poll briefly.
            with open('/proc/%d/uid_map' % worker) as f:
                mapping = [line.split() for line in f.read().strip().splitlines()]
            if mapping != identity:
                time.sleep(0.01)
        require(mapping == identity, 'lane uid mapping is not identity; refusing')
        namespaces = {name: os.readlink('/proc/%d/ns/%s' % (worker, name))
                      for name in ('pid', 'net', 'user', 'mnt', 'ipc', 'uts', 'cgroup')}
        for name, value in namespaces.items():
            require(value != os.readlink('/proc/self/ns/' + name),
                    'lane namespace not private: ' + name)
        # trace:TASK-2708 | ai:claude — the lane netns identity the broker
        # compares each connecting socket's own netns (SIOCGSKNS) against.
        lane_md = os.stat('/proc/%d/ns/net' % worker)
        lane_net = (lane_md.st_dev, lane_md.st_ino)
        user_md = os.stat('/proc/%d/ns/user' % worker)
        lane_user = (user_md.st_dev, user_md.st_ino)
        os.write(block_w, b'R')
        os.close(block_w)
        block_w = None
        if broker_mode:
            broker.settimeout(20)
            data, got = receive(broker)
            require(data == b'listener-v1' and len(got) == 1, 'broker listener unavailable')
            notification = got[0]
            os.close(os.pidfd_open(process.pid))
            broker.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
            broker.send(b'ready-v1')
        # trace:TASK-2707 | ai:codex
        # Service trapped preflight calls BEFORE attesting the shim. The kernel
        # supplies its host PID via SCM_CREDENTIALS (never an env/JSON claim).
        end = time.monotonic() + 65
        while True:
            require(time.monotonic() < end and process.poll() is None,
                    'contained requested-seat preflight did not complete')
            ready, _, _ = select.select([broker, notification, proxy], [], [], 0.1)
            if notification in ready:
                broker_connect_contained(notification, state_fd, state_prefix, connector, port,
                                         lane_net, lane_user)
            if proxy in ready:
                data, got = receive(proxy)
                require(data, 'proxy died during preflight')
                threading.Thread(target=proxy_request, args=(data, got, allowed, fixture),
                                 daemon=True).start()
            if broker in ready:
                data, ancillary, flags, _ = broker.recvmsg(32, socket.CMSG_SPACE(12))
                require(data == b'preflight-v1' and not flags, 'preflight receipt refused')
                credentials = [struct.unpack('3i', value) for level, kind, value in ancillary
                               if level == socket.SOL_SOCKET and kind == socket.SCM_CREDENTIALS]
                require(len(credentials) == 1, 'shim PID attestation unavailable')
                shim, uid, _ = credentials[0]
                require(shim > 0 and uid == os.getuid(), 'shim identity refused')
                with open('/proc/%d/uid_map' % shim) as f:
                    require([line.split() for line in f] == identity, 'shim uid mapping refused')
                require(all(os.readlink('/proc/%d/ns/%s' % (shim, name)) == value
                            for name, value in namespaces.items()), 'shim namespace refused')
                with open('/proc/%d/stat' % shim) as f:
                    start = f.read().rsplit(')', 1)[1].split()[19]
                break
        receipt = dict(version=1, posture='aida', worker_pid=shim, worker_start=start,
                       wrapper_pid=process.pid, namespaces=namespaces,
                       uid_map_identity=True, seat_preflight=True,
                       broker='seccomp_notify_addfd', relay_port=port,
                       policy_sha=cfg['policy_sha'], keyring='isolated_child_only')
        os.write(receipt_fd, json.dumps(receipt).encode() + b'\n')
        os.close(receipt_fd)
        receipt_fd = None
        ready, _, _ = select.select([acceptance_fd], [], [], 10)
        require(ready and os.read(acceptance_fd, 1) == b'R', 'receipt was not accepted')
        os.close(acceptance_fd)
        broker.send(b'exec-v1')
        watch = [proxy] + ([notification] if notification is not None else [])
        while process.poll() is None:
            ready, _, _ = select.select(watch, [], [], 0.2)
            if notification in ready and not broker_connect_contained(
                    notification, state_fd, state_prefix, connector, port, lane_net, lane_user):
                if select.poll and _hung_up(notification):
                    watch.remove(notification)
            if proxy in ready:
                data, got = receive(proxy)
                if not data:
                    watch.remove(proxy)
                    continue
                threading.Thread(target=proxy_request, args=(data, got, allowed, fixture),
                                 daemon=True).start()
        rc = process.wait()
        return rc if rc >= 0 else 128 - rc
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        for fd in (notification, receipt_fd, block_w, info_r, state_fd):
            if fd is not None:
                try:
                    os.close(fd)
                except OSError:
                    pass
        for s in (broker, proxy, connector):
            s.close()

