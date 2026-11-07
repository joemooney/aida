"""Read-only lane client; never substitutes a new ID after uncertain delivery.
trace:TASK-3225 | ai:codex
"""
import uuid


def mutation_probe(endpoint):
    epoch = endpoint.split('/')[-2]
    if str(uuid.UUID(epoch)) != epoch or endpoint != '/run/aida-mutation/%s/service' % epoch:
        raise RuntimeError('invalid store service endpoint')
    request_id = str(uuid.uuid4())
    challenge = str(uuid.uuid4())
    raw = json.dumps(dict(version=1, launch_epoch=epoch, request_id=request_id,
                          op='session_probe', data=dict(challenge=challenge))).encode()
    for attempt in range(3):
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET | socket.SOCK_CLOEXEC) as channel:
                channel.settimeout(5)
                channel.connect(endpoint)
                # write preserves the actual sender's SCM_CREDENTIALS. sendmsg
                # is mediated by the generic broker and cannot prove this.
                require(os.write(channel.fileno(), raw) == len(raw), 'short service request')
                reply, ancillary, flags, _ = channel.recvmsg(65536, socket.CMSG_SPACE(256), socket.MSG_CMSG_CLOEXEC)
                for level, kind, value in ancillary:
                    if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                        for fd in array.array('i', value):
                            os.close(fd)
                require(not flags & (socket.MSG_TRUNC | socket.MSG_CTRUNC) and not ancillary, 'invalid service response')
                obj = json.loads(reply)
                require(set(obj) == {'version', 'launch_epoch', 'request_id', 'outcome', 'result'}
                        and obj['version'] == 1 and obj['launch_epoch'] == epoch
                        and obj['request_id'] == request_id and obj['outcome'] == 'success'
                        and obj['result']['challenge'] == challenge,
                        'store service probe refused')
                return
        except (OSError, ValueError):
            if attempt == 2:
                raise RuntimeError('store service unavailable') from None
            time.sleep(0.05 * (attempt + 1))
