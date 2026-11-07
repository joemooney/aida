
# ---- contained_lane.py: in-lane shim for a contained vendor launch --------
# Appended to egress_probe.py (shared typed-channel helpers) and run as fixed
# code by /usr/bin/python3 -I -S -c inside the outer bwrap. Never reads a
# script from a mutable mount. trace:TASK-2707 | ai:claude
#
# Order: bind the in-netns relay listener -> double-fork the relay helper
# (outside the filter, reparented to the namespace init, not dumpable) ->
# install the connect filter on THIS process -> hand the notification
# listener to the host broker -> wait for broker readiness -> run the seat
# preflight under the filter -> exec the vendor. No model work before ready.

O_NONBLOCK_FLAG = 0o4000
NETLINK_ROUTE = 0
PR_SET_DUMPABLE = 4


def lane_filter():
    """x86-64 seccomp program for the contained worker and every descendant.

    connect() is always a user notification, never CONTINUE: the host
    broker reads the sockaddr once and installs a socket it chose, or (for
    a socket the kernel says lives in a nested netns) connects that same
    socket itself (see broker_connect_contained).
    socket(): AF_UNIX STREAM/SEQPACKET, AF_INET/AF_INET6 STREAM/DGRAM, and
    AF_NETLINK protocol NETLINK_ROUTE only. A unix DGRAM socket could
    sendto() a host path past connect brokering, so it stays denied. Inet
    DGRAM reaches nothing: the lane netns has only lo and no route.
    NETLINK_ROUTE lets a nested netns bring up its own lo (the lane netns
    itself is cap-dropped). RAW inet and every other netlink protocol stay
    denied. socketpair(): AF_UNIX, any type. sendto/sendmsg are brokered
    with copied arguments, refusing named AF_UNIX destinations and FASTOPEN;
    sendmmsg is refused. A mutable msghdr is never continued.
    trace:TASK-2708 | ai:codex ptrace and cross-process
    memory/fd access are refused so a worker cannot drive the unfiltered
    relay helper. io_uring and setns are refused (connect/namespace bypass).
    """
    require(platform.machine() == 'x86_64', 'socket broker architecture unsupported')
    LD, JEQ, JSET, AND, RET = 0x20, 0x15, 0x45, 0x54, 0x06
    prog, fixups = [], []

    def emit(code, k=0, jt=None, jf=None):
        prog.append([code, jt, jf, k])

    def label(name):
        prog.append(('label', name))

    emit(LD, 4)
    emit(JEQ, 0xc000003e, 'arch_ok', 'kill')
    label('arch_ok')
    emit(LD, 0)
    emit(JSET, 0x40000000, 'deny', 'nr_ok')  # x32 ABI
    label('nr_ok')
    for nr in (425, 426, 427, 308, 438, 101, 310, 311, 312, 307):
        emit(JEQ, nr, 'deny', None)
    emit(JEQ, 42, 'notify', None)
    emit(JEQ, 41, 'sock', None)
    emit(JEQ, 53, 'pair', None)
    emit(JEQ, 44, 'notify', None)
    emit(JEQ, 46, 'notify', 'allow')
    label('sock')
    emit(LD, 16)
    emit(JEQ, socket.AF_UNIX, 'local_type', None)
    emit(JEQ, socket.AF_INET, 'inet_type', None)
    emit(JEQ, socket.AF_INET6, 'inet_type', None)
    emit(JEQ, socket.AF_NETLINK, 'netlink_protocol', 'deny')
    label('pair')
    emit(LD, 16)
    emit(JEQ, socket.AF_UNIX, 'allow', 'deny')
    label('local_type')
    emit(LD, 24)
    emit(AND, 0xf)
    emit(JEQ, socket.SOCK_STREAM, 'allow', None)
    emit(JEQ, socket.SOCK_SEQPACKET, 'allow', 'deny')
    label('inet_type')
    emit(LD, 24)
    emit(AND, 0xf)
    emit(JEQ, socket.SOCK_STREAM, 'allow', None)
    emit(JEQ, socket.SOCK_DGRAM, 'allow', 'deny')
    label('netlink_protocol')
    emit(LD, 16 + 8 * 2)
    emit(JEQ, NETLINK_ROUTE, 'allow', 'deny')
    label('notify')
    emit(RET, NOTIFY)
    label('allow')
    emit(RET, ALLOW)
    label('deny')
    emit(RET, DENY)
    label('kill')
    emit(RET, 0x80000000)
    # Resolve labels; None means fall through to the next instruction.
    where, index = {}, 0
    for item in prog:
        if item[0] == 'label':
            where[item[1]] = index
        else:
            index += 1
    ops, index = [], 0
    for item in prog:
        if item[0] == 'label':
            continue
        code, jt, jf, k = item
        def off(target):
            if target is None:
                return 0
            d = where[target] - index - 1
            require(0 <= d < 256, 'filter jump out of range')
            return d
        ops.append((code, off(jt), off(jf), k))
        index += 1
    return ops


def install_lane_filter():
    class Filter(C.Structure):
        _fields_ = [('code', C.c_ushort), ('jt', C.c_ubyte),
                    ('jf', C.c_ubyte), ('k', C.c_uint)]
    class Program(C.Structure):
        _fields_ = [('len', C.c_ushort), ('filter', C.POINTER(Filter))]
    ops = lane_filter()
    filters = (Filter * len(ops))(*(Filter(*op) for op in ops))
    prog = Program(len(ops), filters)
    checked(LIBC.prctl(38, 1, 0, 0, 0), 'no_new_privs unavailable')
    # Handled signals must not restart a send after the host performed its side effect.
    # No fallback: flags unsupported by the kernel refuse before vendor exec.
    # trace:TASK-2708 | ai:codex
    return checked(LIBC.syscall(317, 1, 8 | 32, C.byref(prog)),
                   'SECCOMP_WAIT_KILLABLE_RECV_UNAVAILABLE: requires Linux 5.19+ NEW_LISTENER|WAIT_KILLABLE_RECV')


def splice(a, b):
    def pump(src, dst):
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    break
                dst.sendall(data)
        except OSError:
            pass
        for s, how in ((dst, socket.SHUT_WR), (src, socket.SHUT_RD)):
            try:
                s.shutdown(how)
            except OSError:
                pass
    t = threading.Thread(target=pump, args=(b, a), daemon=True)
    t.start()
    pump(a, b)
    t.join()
    a.close()
    b.close()


def relay_client(conn, proxy):
    """One HTTP CONNECT exchange. Only CONNECT is served; the host decides."""
    try:
        conn.settimeout(30)
        head = b''
        while b'\r\n\r\n' not in head:
            chunk = conn.recv(4096)
            require(chunk and len(head) + len(chunk) <= 8192, 'proxy request bound')
            head += chunk
        head, rest = head.split(b'\r\n\r\n', 1)
        lines = head.decode('latin-1').split('\r\n')
        parts = lines[0].split(' ')
        if len(parts) != 3 or parts[0] != 'CONNECT' or not parts[2].startswith('HTTP/1.'):
            conn.sendall(b'HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n')
            return conn.close()
        authority = parts[1]
        host, _, port = authority.rpartition(':')
        header = authority
        for line in lines[1:]:
            name, _, value = line.partition(':')
            if name.strip().lower() == 'host':
                header = value.strip()
        try:
            port = int(port)
        except ValueError:
            port = -1
        mine, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        request = dict(version=1, method='CONNECT', host=host, port=port,
                       host_header=header, protocol='tcp')
        send_fd(proxy, json.dumps(request).encode(), theirs.fileno())
        theirs.close()
        mine.settimeout(40)
        reply, fds = receive(mine)
        mine.close()
        if reply != b'tunnel-v1' or len(fds) != 1:
            for fd in fds:
                os.close(fd)
            conn.sendall(b'HTTP/1.1 403 Forbidden\r\nX-Aida-Containment: destination refused\r\n'
                         b'Connection: close\r\n\r\n')
            return conn.close()
        tunnel = socket.socket(fileno=fds[0])
        tunnel.settimeout(None)
        conn.settimeout(None)
        conn.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
        if rest:
            tunnel.sendall(rest)
        splice(conn, tunnel)
    except Exception:
        try:
            conn.close()
        except OSError:
            pass


def relay(listener, proxy, connector, port):
    """Unfiltered in-netns relay. Its only peers: the host proxy channel and
    the host broker's connector channel. Not dumpable, so no same-uid
    worker can read or write its memory, and not a worker descendant."""
    checked(LIBC.prctl(PR_SET_DUMPABLE, 0, 0, 0, 0), 'relay dumpable flag')
    def connector_loop():
        while True:
            data = connector.recv(64)
            if data != b'connect-v1':
                os._exit(0)
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.connect(('127.0.0.1', port))
                send_fd(connector, b'connected-v1', s.fileno())
    threading.Thread(target=connector_loop, daemon=True).start()
    while True:
        conn, _ = listener.accept()
        threading.Thread(target=relay_client, args=(conn, proxy), daemon=True).start()


def seat_preflight(argv):
    """Run the requested-seat probe inside the boundary, under the filter."""
    process = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, timeout=60)
    lines = process.stdout[-4096:].decode(errors='replace').splitlines()
    require(process.returncode == 0 and
            'seat_grant_valid=true expected_seat=true' in lines,
            'contained requested-seat preflight refused')


# A passing outer bwrap does not prove Claude can start its own sandbox.
# Probe under the installed lane filter/capability floor, in a disposable child.
# trace:TASK-2708 | ai:codex
def probe_nested_userns():
    pid = os.fork()
    if pid == 0:
        rc = LIBC.unshare(0x10000000 | 0x40000000)
        code = C.get_errno() if rc < 0 else 0
        os._exit(min(code, 255))
    _, status = os.waitpid(pid, 0)
    require(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0,
            'NESTED_USERNS_UNAVAILABLE: lane unshare(CLONE_NEWUSER|CLONE_NEWNET) '
            'refused (errno/status %d); host must enable nested user namespaces' %
            (os.WEXITSTATUS(status) if os.WIFEXITED(status) else status))


def lane_args(argv):
    return (int(argv[1]), int(argv[2]), int(argv[3]), int(argv[4]),
            json.loads(argv[5]), json.loads(argv[6]), argv[7:])


# Interactive provenance queries run only after the host accepts the receipt.
# The child has private stdio and no seat/identity selectors. Match the legacy
# two-second / 257-byte probe, keeping the group leader unreaped through cleanup.
# trace:TASK-3219 | ai:codex
def interactive_harness_version(program):
    import unicodedata, signal
    # Preserve an unreaped group leader until cleanup; auto-reaping would
    # invalidate that PID reservation. An unusual disposition yields unknown.
    if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
        return None
    env = {k: v for k, v in os.environ.items()
           if not k.startswith('AIDA_') or k in ('AIDA_CONTAINED', 'AIDA_CONTAINMENT_POSTURE')}
    process = None
    value = None
    try:
        process = subprocess.Popen([program, '--version'], env=env,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, start_new_session=True)
        os.set_blocking(process.stdout.fileno(), False)
        end, data, eof = time.monotonic() + 2, b'', False
        while time.monotonic() < end:
            try:
                chunk = os.read(process.stdout.fileno(), 258)
                eof = not chunk
                data += chunk
                if len(data) > 257:
                    break
            except BlockingIOError:
                pass
            exited = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOWAIT | os.WNOHANG)
            if eof and exited is not None:
                value = data
                break
            time.sleep(0.01)
    except (OSError, ValueError):
        pass
    finally:
        if process is not None:
            try:
                os.killpg(process.pid, 9)
            except ProcessLookupError:
                pass
            status = process.wait()
            process.stdout.close()
            if status != 0:
                value = None
    if value is not None:
        try:
            value = value.decode('utf-8').strip()
            if value and len(value.encode('utf-8')) <= 256 and not any(
                    unicodedata.category(c) == 'Cc' for c in value):
                return value
        except UnicodeError:
            pass
    return None


def contained_main(broker_fd, proxy_fd, connector_fd, port, preflight, nested_userns, argv):
    # broker_fd < 0 exists only for the test-only positive-control lane, which
    # Rust can request solely under cfg(test). trace:TASK-2707 | ai:claude
    # A private PTY is supplied only as stdio; bwrap keeps --new-session and /dev.
    # Claim it inside the lane, never the host's controlling terminal.
    # trace:TASK-3219 | ai:codex
    if os.environ.get('AIDA_HEADLESS') == '0':
        require(os.isatty(0) and os.isatty(1) and os.isatty(2), 'private PTY required')
        import fcntl, termios
        if os.getsid(0) != os.getpid():
            os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    broker = socket.socket(fileno=broker_fd) if broker_fd >= 0 else None
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(('127.0.0.1', port))
    listener.listen(128)
    first = os.fork()
    if first == 0:
        # Double fork: the relay is reparented to the namespace init and is
        # never a descendant of the vendor (yama ptrace scope 1 and the
        # filter's ptrace denial both apply).
        if broker is not None:
            broker.close()
        if os.fork() != 0:
            os._exit(0)
        try:
            relay(listener, socket.socket(fileno=proxy_fd),
                  socket.socket(fileno=connector_fd), port)
        finally:
            os._exit(0)
    os.waitpid(first, 0)
    listener.close()
    os.close(proxy_fd)
    os.close(connector_fd)
    if broker is not None:
        # Transfer the new listener from a trusted bootstrap thread created
        # before filtering. Otherwise the very first sendmsg would trap before
        # the host could receive the listener. Join it before any worker exec.
        # trace:TASK-2708 | ai:codex
        ready = threading.Event()
        handoff = []
        def transfer_listener():
            ready.wait()
            send_fd(broker, b'listener-v1', handoff[0])
        transfer = threading.Thread(target=transfer_listener)
        transfer.start()
        notification = install_lane_filter()
        handoff.append(notification)
        ready.set()
        transfer.join()
        os.close(notification)
        broker.settimeout(20)
        require(broker.recv(32) == b'ready-v1', 'broker readiness refused')
    if nested_userns:
        probe_nested_userns()
    if preflight:
        seat_preflight(preflight)
    # trace:TASK-3225 | ai:codex — exercise the actual injected lane channel.
    endpoint = os.environ.get('AIDA_STORE_MUTATION_ENDPOINT')
    if endpoint is not None:
        mutation_probe(endpoint)
    # trace:TASK-2707 | ai:codex — no vendor exec before host acceptance.
    require(broker is not None, 'production receipt requires broker')
    # write(2) preserves the shim's SCM_CREDENTIALS; brokered sendto would
    # instead attach the host supervisor's identity.
    os.write(broker.fileno(), b'preflight-v1')
    require(broker.recv(32) == b'exec-v1', 'receipt acceptance refused')
    broker.close()
    if os.environ.get('AIDA_HEADLESS') == '0':
        os.environ.pop('AIDA_HARNESS_VERSION', None)
        version = interactive_harness_version(argv[0])
        if version is not None:
            os.environ['AIDA_HARNESS_VERSION'] = version
    os.execve(argv[0], argv, dict(os.environ))
