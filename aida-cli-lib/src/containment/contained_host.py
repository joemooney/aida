
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


def broker_connect_contained(listener, state_fd, state_prefix, connector, port):
    """F-a broker: read the sockaddr ONCE, never CONTINUE. Relay port on
    127.0.0.1 gets a relay-connected socket; a pathname beneath the private
    state root gets the pinned private endpoint; everything else refuses."""
    n = Notification()
    if LIBC.ioctl(listener, C.c_ulonglong(RECV), C.byref(n)) < 0:
        # ENOENT: the trapped task was interrupted or every filtered task is
        # gone (the listener reports hang-up). Nothing to answer.
        require(C.get_errno() in (errno.ENOENT, errno.EINTR),
                'seccomp notification facility unavailable')
        return False
    require(n.data.arch == 0xc000003e and n.data.nr == 42 and n.flags == 0,
            'unexpected socket broker notification')
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
        if family == socket.AF_INET:
            answer = ECONNREFUSED_ERR
            require(length >= 8, 'short inet address')
            dport = struct.unpack('!H', address[2:4])[0]
            if address[4:8] == b'\x7f\x00\x00\x01' and dport == port:
                connector.send(b'connect-v1')
                reply, fds = receive(connector)
                require(reply == b'connected-v1' and len(fds) == 1, 'relay connector refused')
                connected = socket.socket(fileno=fds[0])
        elif family == socket.AF_INET6:
            answer = ECONNREFUSED_ERR
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
        str(connector_child.fileno()), str(port), json.dumps(cfg['preflight'])] + cfg['argv']
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
                broker_connect_contained(notification, state_fd, state_prefix, connector, port)
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
                    notification, state_fd, state_prefix, connector, port):
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

