"""Closed slice-B fixture, embedded by Rust; never a vendor/command runner.

All endpoints, policies, payloads and commands are synthetic and fixed. The
only arguments are the descriptor-pinned slice-A bwrap vector, private fixture
root, info fd and a closed fault selector supplied by the supervisor.
trace:TASK-2706 | ai:codex
"""
import array
import ctypes as C
import errno
import ipaddress
import json
import os
import platform
import select
import socket
import stat
import struct
import subprocess
import sys
import threading
import time

LIBC = C.CDLL(None, use_errno=True)
LIMIT = 4096
RECV, SEND, VALID, ADDFD = 0xc0502100, 0xc0182101, 0x40082102, 0x40182103
DENY, ALLOW, NOTIFY = 0x50000 | errno.EACCES, 0x7fff0000, 0x7fc00000


def require(value, message):
    if not value:
        raise RuntimeError(message)


def checked(rc, message):
    if rc < 0:
        raise RuntimeError(message + ' (errno %d)' % C.get_errno())
    return rc


def ioctl(fd, request, value):
    return checked(LIBC.ioctl(fd, C.c_ulong(request), C.byref(value)),
                   'seccomp notification facility unavailable')


class Data(C.Structure):
    _fields_ = [('nr', C.c_int), ('arch', C.c_uint), ('ip', C.c_ulonglong),
                ('args', C.c_ulonglong * 6)]


class Notification(C.Structure):
    _fields_ = [('id', C.c_ulonglong), ('pid', C.c_uint), ('flags', C.c_uint),
                ('data', Data)]


class Response(C.Structure):
    _fields_ = [('id', C.c_ulonglong), ('val', C.c_longlong),
                ('error', C.c_int), ('flags', C.c_uint)]


class Addfd(C.Structure):
    _fields_ = [('id', C.c_ulonglong), ('flags', C.c_uint), ('srcfd', C.c_uint),
                ('newfd', C.c_uint), ('newfd_flags', C.c_uint)]


def install_filter():
    require(platform.machine() == 'x86_64', 'socket broker architecture unsupported')
    class Filter(C.Structure):
        _fields_ = [('code', C.c_ushort), ('jt', C.c_ubyte),
                    ('jf', C.c_ubyte), ('k', C.c_uint)]
    class Program(C.Structure):
        _fields_ = [('len', C.c_ushort), ('filter', C.POINTER(Filter))]
    # Arch check, then reject x32 before matching x86-64 syscall numbers.
    ops = [(0x20, 0, 0, 4), (0x15, 1, 0, 0xc000003e), (0x06, 0, 0, 0x80000000),
           (0x20, 0, 0, 0), (0x35, 0, 1, 0x40000000), (0x06, 0, 0, DENY)]
    # No io_uring connect/socket bypass, namespace switching or fd stealing.
    for nr in [425, 426, 427, 308, 438]:
        ops += [(0x15, 0, 1, nr), (0x06, 0, 0, DENY)]
    ops += [(0x15, 0, 1, 42), (0x06, 0, 0, NOTIFY)]  # connect
    # socket/socketpair: only local STREAM/SEQPACKET. DGRAM can sendto a
    # pathname without connect; all internet/netlink/raw sockets are denied.
    ops += [(0x15, 1, 0, 41), (0x15, 0, 9, 53),
            (0x20, 0, 0, 16), (0x15, 1, 0, socket.AF_UNIX),
            (0x06, 0, 0, DENY), (0x20, 0, 0, 24),
            (0x54, 0, 0, 0xf), (0x15, 2, 0, socket.SOCK_STREAM),
            (0x15, 1, 0, socket.SOCK_SEQPACKET), (0x06, 0, 0, DENY),
            (0x06, 0, 0, ALLOW), (0x06, 0, 0, ALLOW)]
    filters = (Filter * len(ops))(*(Filter(*op) for op in ops))
    prog = Program(len(ops), filters)
    checked(LIBC.prctl(38, 1, 0, 0, 0), 'no_new_privs unavailable')
    return checked(LIBC.syscall(317, 1, 8, C.byref(prog)),
                   'seccomp NEW_LISTENER unavailable')


def send_fd(channel, payload, fd):
    channel.sendmsg([payload], [(socket.SOL_SOCKET, socket.SCM_RIGHTS,
                                array.array('i', [fd]))])


def receive(channel):
    data, anc, flags, _ = channel.recvmsg(LIMIT, socket.CMSG_SPACE(4 * 8),
                                        socket.MSG_CMSG_CLOEXEC)
    fds = []
    for level, kind, value in anc:
        if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
            a = array.array('i')
            a.frombytes(value[:len(value) - len(value) % a.itemsize])
            fds.extend(a)
    if flags & (socket.MSG_TRUNC | socket.MSG_CTRUNC):
        for fd in fds:
            os.close(fd)
        raise RuntimeError('typed channel exceeds bound')
    return data, fds


def host_name(name):
    require(isinstance(name, str) and 0 < len(name) <= 253,
            'invalid proxy authority')
    require(name == name.lower() and not name.endswith('.'), 'ambiguous proxy authority')
    require(all(0 < len(p) <= 63 and p[0].isalnum() and p[-1].isalnum()
                and all(c in 'abcdefghijklmnopqrstuvwxyz0123456789-' for c in p)
                for p in name.split('.')), 'invalid proxy authority')
    try:
        ipaddress.ip_address(name)
    except ValueError:
        return name
    raise RuntimeError('IP literal proxy authority refused')


def destination(request, allowed, resolve, fixture=None):
    # Decode no ambient proxy env and accept no upstream/URL/path/header bag.
    require(type(request) is dict and set(request) == {
        'version', 'method', 'host', 'port', 'host_header', 'protocol'},
        'invalid typed proxy request')
    require(type(request['version']) is int and request['version'] == 1 and
            request['method'] == 'CONNECT' and request['protocol'] == 'tcp',
            'unsupported proxy protocol')
    name, port = host_name(request['host']), request['port']
    require(type(port) is int and 0 < port < 65536 and (name, port) in allowed,
            'proxy destination refused')
    require(request['host_header'] == '%s:%d' % (name, port),
            'proxy Host authority mismatch')
    answers = resolve(name, port)
    require(0 < len(answers) <= 16, 'proxy DNS answer refused')
    pinned = []
    for family, kind, proto, _, address in answers:
        ip = ipaddress.ip_address(address[0])
        require(family in (socket.AF_INET, socket.AF_INET6) and
                kind == socket.SOCK_STREAM and proto == socket.IPPROTO_TCP,
                'proxy DNS protocol refused')
        require(address[1] == port, 'proxy DNS port mismatch')
        # Only the host-created fixture tuple can opt into loopback. This is
        # never derived from config, the child, proxy env or a DNS response.
        require(ip.is_global or (fixture == (name, port, str(ip))),
                'proxy private/rebinding address refused')
        require(not (getattr(ip, 'ipv4_mapped', None) or ip.is_multicast),
                'proxy mapped/multicast address refused')
        pinned.append((family, kind, proto, address))
    return pinned[0]  # connect exactly this checked numeric sockaddr; no second lookup


def policy_checks():
    request = dict(version=1, method='CONNECT', host='fixture.example', port=443,
                   host_header='fixture.example:443', protocol='tcp')
    allowed = {('fixture.example', 443)}
    def resolve(ip):
        return lambda *_: [(socket.AF_INET, socket.SOCK_STREAM,
                            socket.IPPROTO_TCP, '', (ip, 443))]
    require(destination(request, allowed, resolve('93.184.216.34'))[-1] ==
            ('93.184.216.34', 443), 'numeric destination pinning')
    for changes in [dict(host='other.example'), dict(host='127.0.0.1'),
                    dict(host='fixture.example.'), dict(host='FIXTURE.example'),
                    dict(host='user@fixture.example'), dict(host='*.example'),
                    dict(host='[::1]'), dict(host='2130706433'),
                    dict(port=80), dict(port=True), dict(protocol='udp'),
                    dict(method='GET'), dict(host_header='other.example:443'),
                    dict(upstream='http://ambient.invalid')]:
        try:
            destination(dict(request, **changes), allowed, resolve('93.184.216.34'))
        except RuntimeError:
            continue
        raise RuntimeError('proxy authority negative control accepted')
    for ip in ['127.0.0.1', '10.0.0.1', '169.254.169.254', '0.0.0.0',
               '192.168.0.1', '224.0.0.1', '100.64.0.1']:
        try:
            destination(request, allowed, resolve(ip))
        except RuntimeError:
            continue
        raise RuntimeError('private/rebinding control accepted')
    public = resolve('93.184.216.34')('fixture.example', 443)
    private = resolve('127.0.0.1')('fixture.example', 443)
    bad_answers = [public + private]
    for ip in ['::1', 'fd00::1', 'fe80::1', 'ff02::1', '::ffff:93.184.216.34']:
        bad_answers.append([(socket.AF_INET6, socket.SOCK_STREAM,
                             socket.IPPROTO_TCP, '', (ip, 443, 0, 0))])
    for answers in bad_answers:
        try:
            destination(request, allowed, lambda *_: answers)
        except RuntimeError:
            continue
        raise RuntimeError('mixed/IPv6 DNS negative control accepted')
    try:
        destination(request, set(), resolve('93.184.216.34'))
    except RuntimeError:
        return
    raise RuntimeError('empty policy accepted')


def broker_connect(listener, runtime):
    n = Notification()
    ioctl(listener, RECV, n)
    require(n.data.arch == 0xc000003e and n.data.nr == 42 and n.flags == 0,
            'unexpected socket broker notification')
    ident = C.c_ulonglong(n.id)
    # Pin the task's memory handle BEFORE validating its notification ID.
    # Read bounded sockaddr once. Never resume a kernel dereference (CONTINUE),
    # and never interpret the child's fd, cwd, dirfd or path in the host.
    denied_errno = -errno.EACCES
    try:
        length = n.data.args[2]
        require(3 <= length <= 110, 'socket address exceeds bound')
        require(n.data.args[0] < 2**31, 'socket descriptor exceeds bound')
        mem = os.open('/proc/%d/mem' % n.pid, os.O_RDONLY | os.O_CLOEXEC)
        try:
            ioctl(listener, VALID, ident)
            address = os.pread(mem, length, n.data.args[1])
        finally:
            os.close(mem)
        ioctl(listener, VALID, ident)
        require(len(address) == length and address[:2] == struct.pack('H', socket.AF_UNIX),
                'non-local connect refused')
        # Closed synthetic contract: precisely one private own endpoint. NUL
        # aliases, abstract names, relative paths and every host path refuse.
        require(address[2:].rstrip(b'\0') == b'/lane/state/private.sock' and
                b'\0' not in address[2:].rstrip(b'\0'), 'non-private IPC refused')
        target = os.open('private.sock', os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC,
                         dir_fd=runtime)
        try:
            md = os.fstat(target)
            require(stat.S_ISSOCK(md.st_mode) and md.st_uid == os.getuid()
                    and md.st_nlink == 1, 'private endpoint identity refused')
            connected = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                connected.settimeout(1)
                connected.connect('/proc/self/fd/%d' % target)
                # Never look up or act on the child's fd. SETFD replaces its
                # slot with this safe connected socket. SEND then returns the
                # connect ABI's zero (ADDFD_FLAG_SEND returns the fd number and
                # is unsuitable for connect). Cancellation or an fd swap may
                # leave only this approved endpoint; it cannot select another
                # host object. Never CONTINUE or replay mutable arguments.
                ioctl(listener, VALID, ident)
                ioctl(listener, ADDFD, Addfd(n.id, 1, connected.fileno(),
                                             n.data.args[0], os.O_CLOEXEC))
                ioctl(listener, SEND, Response(n.id, 0, 0, 0))
                return
            finally:
                connected.close()
        finally:
            os.close(target)
    except RuntimeError as error:
        if str(error).startswith('seccomp notification facility unavailable'):
            raise  # Missing/failed notification ioctls stop the entire lane.
    except OSError as error:
        if error.errno not in (errno.ENOENT, errno.ELOOP, errno.ECONNREFUSED):
            raise RuntimeError('socket broker memory/endpoint facility refused') from error
    ioctl(listener, SEND, Response(n.id, 0, denied_errno, 0))


def lane(channel_fd, tcp_port, abstract):
    checks = []
    channel = socket.socket(fileno=channel_fd)
    # All host handles except the typed connected channel were closed at the
    # host->bwrap boundary. Do not confuse a reused fd number with provenance.
    for entry in os.listdir('/proc/self/fd'):
        fd = int(entry)
        try:
            mode = os.fstat(fd).st_mode
        except OSError:
            continue
        require(not stat.S_ISSOCK(mode) or fd == channel_fd,
                'untyped inherited host socket reached lane')
    require(all(k not in os.environ for k in
                ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
                 'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy']),
            'ambient proxy environment reached lane')
    listener = install_filter()
    send_fd(channel, b'listener-v1', listener)
    os.close(listener)
    channel.settimeout(3)
    require(channel.recv(32) == b'ready-v1', 'broker readiness refused')
    checks.append('broker_ready_before_lane')
    def denied(name, operation):
        try:
            operation()
        except OSError as e:
            require(e.errno in (errno.EACCES, errno.EPERM), name + ' wrong refusal')
            checks.append(name)
            return
        raise RuntimeError(name + ' escape succeeded')
    for family, kind, name in [(socket.AF_INET, socket.SOCK_STREAM, 'direct_tcp_denied'),
                               (socket.AF_INET6, socket.SOCK_STREAM, 'direct_ipv6_denied'),
                               (socket.AF_INET, socket.SOCK_DGRAM, 'udp_dns_quic_denied'),
                               (socket.AF_UNIX, socket.SOCK_DGRAM, 'unix_sendto_denied'),
                               (socket.AF_NETLINK, socket.SOCK_RAW, 'netlink_denied')]:
        denied(name, lambda f=family, k=kind: socket.socket(f, k))
    denied('unix_dgram_socketpair_denied',
           lambda: socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM))
    denied('io_uring_denied', lambda: checked_os_syscall(425, 1, 0))
    denied('x32_socket_bypass_denied', lambda: checked_os_syscall(0x40000029, 2, 1))
    for path, name in [('/host-services/host.sock', 'host_pathname_socket_denied'),
                       ('/host-services/dbus.sock', 'host_dbus_denied'),
                       ('/host-services/ssh-agent.sock', 'host_ssh_agent_denied'),
                       ('/host-services/app-server-control.sock', 'host_codex_daemon_denied'),
                       ('\0' + abstract, 'host_abstract_socket_denied')]:
        with socket.socket(socket.AF_UNIX) as s:
            denied(name, lambda: s.connect(path))
    for operation, name in [(os.rename, 'host_socket_rename_into_runtime_denied'),
                            (os.link, 'host_socket_link_into_runtime_denied')]:
        try:
            operation('/host-services/host.sock', '/lane/state/private.sock')
        except OSError as e:
            require(e.errno in (errno.EXDEV, errno.EROFS, errno.EACCES, errno.EPERM),
                    'host endpoint alias wrong refusal')
            checks.append(name)
        else:
            raise RuntimeError('host endpoint moved into private runtime')
    # The allowed private endpoint is created by the lane, not the host. The
    # broker pins its socket inode; neither a symlink nor a rename can redirect
    # the host connect away from the inode it checked.
    with socket.socket(socket.AF_UNIX) as server:
        server.bind('/lane/state/private.sock')
        server.listen(4)
        with socket.socket(socket.AF_UNIX) as client:
            client.connect('/lane/state/private.sock')
            with server.accept()[0] as accepted:
                client.sendall(b'private-control')
                require(accepted.recv(32) == b'private-control', 'private IPC failed')
        checks.append('private_pathname_ipc_allowed')
    os.unlink('/lane/state/private.sock')
    # Pointer, pathname and descriptor-slot races run against real broker
    # notifications. Even successful connects may reach only the private
    # listener. The host independently checks forbidden accept queues.
    with socket.socket(socket.AF_UNIX) as race_server:
        race_server.bind('/lane/state/private.sock')
        race_server.listen(64)
        address = C.create_string_buffer(110)
        initial = struct.pack('H', socket.AF_UNIX) + b'/lane/state/private.sock\0'
        C.memmove(address, initial, len(initial))
        stop = threading.Event()
        def mutate():
            paths = [b'/lane/state/private.sock\0', b'/lane/state/host.sock\0']
            while not stop.is_set():
                for path in paths:
                    C.memset(C.addressof(address) + 2, 0, 108)
                    C.memmove(C.addressof(address) + 2, path, len(path))
                    try:
                        os.rename('/lane/state/private.sock', '/lane/state/pinned.sock')
                        os.symlink('/host-services/host.sock', '/lane/state/private.sock')
                        os.unlink('/lane/state/private.sock')
                        os.rename('/lane/state/pinned.sock', '/lane/state/private.sock')
                    except OSError:
                        pass
        racer = threading.Thread(target=mutate)
        racer.start()
        try:
            for _ in range(24):
                with socket.socket(socket.AF_UNIX) as client:
                    LIBC.syscall(42, client.fileno(), C.byref(address), 110)
        finally:
            stop.set()
            racer.join(timeout=1)
            require(not racer.is_alive(), 'sockaddr race fixture did not stop')
        checks.append('mutable_sockaddr_and_rename_races_confined')
        # Race the target fd slot too. The broker never inspects/replays its
        # referent; any installed fd points only at the pinned private socket.
        C.memset(address, 0, 110)
        C.memmove(address, initial, len(initial))
        with socket.socket(socket.AF_UNIX) as client:
            replacement, peer = socket.socketpair()
            stop.clear()
            def replace_fd():
                while not stop.is_set():
                    os.dup2(replacement.fileno(), client.fileno())
            racer = threading.Thread(target=replace_fd)
            racer.start()
            try:
                for _ in range(24):
                    LIBC.syscall(42, client.fileno(), C.byref(address), len(initial))
            finally:
                stop.set()
                racer.join(timeout=1)
                require(not racer.is_alive(), 'fd race fixture did not stop')
                replacement.close()
                peer.close()
        checks.append('target_fd_swap_confined')
    for name in ['private.sock', 'pinned.sock']:
        try:
            os.unlink('/lane/state/' + name)
        except FileNotFoundError:
            pass
    os.symlink('/host-services/host.sock', '/lane/state/private.sock')
    with socket.socket(socket.AF_UNIX) as client:
        denied('private_endpoint_symlink_denied',
               lambda: client.connect('/lane/state/private.sock'))
    os.unlink('/lane/state/private.sock')
    request = dict(version=1, method='CONNECT', host='fixture.example', port=tcp_port,
                   host_header='fixture.example:%d' % tcp_port, protocol='tcp')
    for changes in [dict(host='blocked.example'), dict(port=tcp_port + 1),
                    dict(protocol='udp'), dict(host_header='blocked.example:443'),
                    dict(host='127.0.0.1')]:
        channel.send(json.dumps(dict(request, **changes)).encode())
        reply, fds = receive(channel)
        require(reply == b'denied-v1' and not fds, 'proxy bypass accepted')
    checks.append('host_port_protocol_bypass_denied')
    channel.send(json.dumps(request).encode())
    reply, fds = receive(channel)
    require(reply == b'tunnel-v1' and len(fds) == 1, 'typed proxy tunnel refused')
    with socket.socket(fileno=fds[0]) as tunnel:
        tunnel.settimeout(2)
        tunnel.sendall(b'fixture-control')
        require(tunnel.recv(32) == b'fixture-control', 'allowed fixture unreachable')
        # A tunnel cannot be repurposed to a new destination with connect().
        denied('tunnel_reconnect_denied', lambda: tunnel.connect(('127.0.0.1', tcp_port)))
    checks.extend(['allowed_fixture_via_typed_proxy', 'ambient_proxy_scrubbed'])
    channel.send(b'finished-v1')
    require(channel.recv(32) == b'finished-v1', 'broker completion refused')
    print(json.dumps(checks), flush=True)


def checked_os_syscall(nr, a, b):
    rc = LIBC.syscall(nr, a, b)
    if rc < 0:
        e = C.get_errno()
        raise OSError(e, 'synthetic syscall refused')
    os.close(rc)


def host(bwrap_args, root, info_fd, fault):
    require(fault in ('normal', 'proxy_before', 'proxy_after', 'broker_before',
                      'broker_after', 'timeout', 'supervisor_timeout'), 'invalid fixed fault selector')
    require(platform.machine() == 'x86_64', 'socket broker architecture unsupported')
    abi = LIBC.syscall(444, 0, 0, 1)
    require(abi > 0, 'Landlock capability unavailable; socket-policy selection refused')
    require(abi < 9,
            'Landlock Unix-connect backend required on ABI>=9; broker not selected')
    sizes = (C.c_ushort * 3)()
    checked(LIBC.syscall(317, 3, 0, C.byref(sizes)),
            'seccomp notification size probe unavailable')
    require(list(sizes) == [C.sizeof(Notification), C.sizeof(Response), C.sizeof(Data)],
            'seccomp notification ABI unsupported')
    policy_checks()
    # Host-owned fixture listeners and positive controls. No external traffic.
    tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp.bind(('127.0.0.1', 0))
    tcp.listen(4)
    port = tcp.getsockname()[1]
    abstract = 'aida-egress-fixture-' + os.path.basename(root)
    endpoints = []
    for name in ['host.sock', 'ssh-agent.sock', 'app-server-control.sock', 'dbus.sock']:
        s = socket.socket(socket.AF_UNIX)
        s.bind(root + '/host-services/' + name)
        s.listen(4)
        with socket.socket(socket.AF_UNIX) as control:
            control.connect(root + '/host-services/' + name)
            s.accept()[0].close()
        endpoints.append(s)
    abstract_server = socket.socket(socket.AF_UNIX)
    abstract_server.bind('\0' + abstract)
    abstract_server.listen(4)
    with socket.socket(socket.AF_UNIX) as control:
        control.connect('\0' + abstract)
        abstract_server.accept()[0].close()
    endpoints.append(abstract_server)
    runtime = os.open(root + '/state', os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW)
    proxy, child_channel = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    proxy.settimeout(3)
    # Pass only the mount/info descriptors and this typed channel. An extra
    # non-CLOEXEC host socket is seeded and deliberately excluded from pass_fds.
    leaked, leaked_peer = socket.socketpair()
    os.set_inheritable(leaked.fileno(), True)
    fds = {info_fd, child_channel.fileno()}
    for i, value in enumerate(bwrap_args):
        if value in ('--bind-fd', '--ro-bind-fd', '--block-fd'):
            fds.add(int(bwrap_args[i + 1]))
    source = SOURCE
    command = ['/usr/bin/bwrap'] + bwrap_args + [
        '--info-fd', str(info_fd), '--', '/usr/bin/python3', '-I', '-S', '-c',
        source + '\nlane(int(sys.argv[1]), int(sys.argv[2]), sys.argv[3])',
        str(child_channel.fileno()), str(port), abstract]
    process = subprocess.Popen(command, pass_fds=tuple(fds), env={},
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child_channel.close()
    # No other process should keep the info pipe alive.
    os.close(info_fd)
    notification = None
    try:
        data, fds = receive(proxy)
        require(data == b'listener-v1' and len(fds) == 1, 'broker listener unavailable')
        notification = fds[0]
        # Required facilities are checked before readiness. The live private
        # IPC positive case also requires NOTIF_ADDFD SETFD support.
        pidfd = os.pidfd_open(process.pid)
        os.close(pidfd)
        def fail_component():
            nonlocal notification
            if fault.startswith('proxy_'):
                proxy.close()
            else:
                os.close(notification)
                notification = None
        if fault.endswith('_before'):
            fail_component()
            raise RuntimeError('synthetic ' + fault + ' death before readiness')
        proxy.send(b'ready-v1')
        if fault == 'supervisor_timeout':
            # Rust kills this helper without Python cleanup. bwrap's parent
            # death contract must still destroy the actual namespace init.
            time.sleep(30)
        ready = True
        end = time.monotonic() + (0.3 if fault == 'timeout' else 8)
        complete = False
        while not complete:
            require(time.monotonic() < end, 'synthetic lane deadline')
            require(process.poll() is None, 'lane exited before broker completion')
            events, _, _ = select.select([proxy, notification], [], [], 0.05)
            if events and ready and fault.endswith('_after'):
                fail_component()
                raise RuntimeError('synthetic ' + fault + ' death after readiness')
            if notification in events:
                broker_connect(notification, runtime)
            if proxy in events:
                data, fds = receive(proxy)
                for fd in fds:
                    os.close(fd)
                if not data:
                    _, detail = process.communicate(timeout=1)
                    # The lane runs only the immutable fixture and cannot
                    # contain user secrets. Bound even these diagnostics.
                    raise RuntimeError('fixed lane closed typed channel: ' +
                                       detail[-1024:].decode(errors='replace'))
                require(not fds, 'untyped proxy fd refused')
                if data == b'finished-v1':
                    if fault == 'timeout':
                        continue
                    proxy.send(b'finished-v1')
                    complete = True
                else:
                    try:
                        def no_duplicates(pairs):
                            d = dict(pairs)
                            require(len(d) == len(pairs), 'duplicate proxy field')
                            return d
                        request = json.loads(data, object_pairs_hook=no_duplicates)
                        def resolve(name, p):
                            return [(socket.AF_INET, socket.SOCK_STREAM,
                                     socket.IPPROTO_TCP, '', ('127.0.0.1', p))]
                        family, kind, proto, address = destination(
                            request, {('fixture.example', port)}, resolve,
                            ('fixture.example', port, '127.0.0.1'))
                    except (RuntimeError, ValueError, TypeError):
                        proxy.send(b'denied-v1')
                        continue
                    with socket.socket(family, kind, proto) as tunnel:
                        tunnel.settimeout(1)
                        tunnel.connect(address)
                        send_fd(proxy, b'tunnel-v1', tunnel.fileno())
                    # Fixed fixture echo, bounded; no relay to arbitrary hosts.
                    with tcp.accept()[0] as accepted:
                        accepted.settimeout(1)
                        payload = accepted.recv(32)
                        require(payload == b'fixture-control', 'fixture payload invalid')
                        accepted.sendall(payload)
        output, error = process.communicate(timeout=2)
        require(process.returncode == 0 and len(output) < LIMIT and len(error) < LIMIT,
                'fixed egress lane refused')
        checks = json.loads(output)
        # Every forbidden host listener must still have an empty accept queue.
        require(not select.select(endpoints, [], [], 0)[0], 'host IPC escape observed')
        checks += ['proxy_policy_negative_controls', 'host_ipc_positive_controls',
                   'broker_addfd_without_continue', 'inherited_host_socket_absent']
        print(json.dumps(checks), flush=True)
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=2)
        if notification is not None:
            os.close(notification)
        os.close(runtime)
        proxy.close()
        tcp.close()
        for s in endpoints:
            s.close()
        leaked.close()
        leaked_peer.close()


# Rust supplies SOURCE as a separate argument to immutable -c code, never a
# path in a mutable mount. Only the host entry expression consumes it.
