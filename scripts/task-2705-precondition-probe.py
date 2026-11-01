#!/usr/bin/python3
"""Fixed synthetic diagnostic, NOT the production acceptance fixture.

Review before HOST execution. No source import, arbitrary argv, secrets, keys,
sudo, sysctl writes, host mount changes, or model execution. --nested-proxy is
an evidence label, not an escape from the caller's confinement.
"""
import errno
import json
import os
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

PIN = "acb177db657a03c5057ecfc9572f7a6b775ac768"
LIMIT = 16384
WORKER = r'''
import os, sys, json, errno, signal
signal.alarm(5)
def emit(stage, **fields):
    print(json.dumps(dict(stage=stage, **fields)), flush=True)
def small(path, limit=1024):
    try:
        with open(path) as f: s=f.read(limit+1)
        return s if len(s)<=limit else None
    except OSError as e: return dict(errno=e.errno)
def facts(stage):
    d={"uid":os.getuid(), "gid":os.getgid()}
    s=small('/proc/self/status',8192)
    if isinstance(s,str):
        d['status']={k:v.strip() for k,v in (l.split(':',1) for l in s.splitlines() if ':' in l)
                     if k in ('NoNewPrivs','CapEff','CapPrm','CapBnd','CapInh','Seccomp','Seccomp_filters')}
    d['profile']=small('/proc/self/attr/current',256)
    d['maps']={k:small('/proc/self/'+k) for k in ('uid_map','gid_map','setgroups')}
    d['sysctl']={k:small('/proc/sys/'+k.replace('.','/'),64) for k in (
        'kernel.apparmor_restrict_unprivileged_userns','kernel.unprivileged_userns_clone',
        'user.max_user_namespaces','user.max_mnt_namespaces')}
    d['namespaces']={}
    for k in ('user','mnt','pid','net','ipc','uts','cgroup'):
        try: d['namespaces'][k]=os.readlink('/proc/self/ns/'+k)
        except OSError as e: d['namespaces'][k]=dict(errno=e.errno)
    emit(stage, facts=d)
facts('entry_facts')
place, variant, control = sys.argv[1:]
protected='/lane/workspace/.aida'
if place=='composed':
    try:
        with open(protected+'/existing','w') as f: f.write('unexpected')
    except OSError as e:
        emit('protected_write_negative',errno=e.errno,pass_control=e.errno in (errno.EROFS,errno.EACCES,errno.EPERM))
        if e.errno not in (errno.EROFS,errno.EACCES,errno.EPERM): sys.exit(20)
    else:
        emit('protected_write_negative',errno=0,pass_control=False); sys.exit(20)
# FFI exists only in this fixed worker, never in its host controller.
import ctypes
libc=ctypes.CDLL(None,use_errno=True)
libc.mount.argtypes=[ctypes.c_char_p,ctypes.c_char_p,ctypes.c_char_p,ctypes.c_ulong,ctypes.c_void_p]
libc.umount2.argtypes=[ctypes.c_char_p,ctypes.c_int]
def call(stage, function, *args):
    ctypes.set_errno(0)
    rc=function(*args)
    e=ctypes.get_errno() if rc else 0
    emit(stage,errno=e,pass_control=rc==0)
    if rc: sys.exit(21)
def mapping():
    # Diagnostic branch only: production nested worker writes NO maps.
    for name,value in [('setgroups','deny'),('gid_map',str(gid)+' '+str(gid)+' 1'),
                       ('uid_map',str(uid)+' '+str(uid)+' 1')]:
        try:
            with open('/proc/self/'+name,'w') as f: f.write(value+'\n')
        except OSError as e:
            emit('map_'+name,errno=e.errno,pass_control=False); sys.exit(22)
        emit('map_'+name,errno=0,pass_control=True)
uid,gid=os.getuid(),os.getgid()
if variant=='split-mapped':
    call('unshare_user_diagnostic',libc.unshare,0x10000000)
    mapping()
    call('unshare_mnt_diagnostic',libc.unshare,0x00020000)
else:
    call('unshare_exact_combined',libc.unshare,0x10000000|0x00020000)
    if variant=='combined-mapped': mapping()
facts('after_unshare_facts')
if place=='outside':
    # Safety addition only outside bwrap. Inside use the source's exact mount
    # ordering; bwrap already establishes the private outer mount namespace.
    call('make_private_safety',libc.mount,None,b'/',None,16384|262144,None)
else:
    # Exact production ordering: unshare, mkdir, mount, umount, protected umount.
    try: os.mkdir(control)
    except OSError as e:
        emit('mkdir_control',errno=e.errno,pass_control=False); sys.exit(25)
    emit('mkdir_control',errno=0,pass_control=True)
target=os.fsencode(control)
call('mount_positive',libc.mount,b'tmpfs',target,b'tmpfs',0,None)
call('unmount_positive',libc.umount2,target,2)
if place=='composed':
    ctypes.set_errno(0)
    rc=libc.umount2(protected.encode(),2)
    e=ctypes.get_errno() if rc else 0
    good=rc==-1 and e in (errno.EPERM,errno.EINVAL)
    emit('protected_unmount_negative',errno=e,pass_control=good)
    if not good: sys.exit(23)
    try:
        with open(protected+'/existing','w') as f: f.write('unexpected')
    except OSError as e:
        good=e.errno in (errno.EROFS,errno.EACCES,errno.EPERM)
        emit('protected_write_after_negative',errno=e.errno,pass_control=good)
        if not good: sys.exit(24)
    else: emit('protected_write_after_negative',errno=0,pass_control=False); sys.exit(24)
emit('phase_complete',errno=0)
'''


def bounded(argv, env, fds=(), deadline=9):
    """Fixed children only; cap BOTH pipes; do not publish raw stderr."""
    p = subprocess.Popen(argv, env=env, cwd='/', stdin=subprocess.DEVNULL,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         close_fds=True, pass_fds=tuple(fds), start_new_session=True)
    sel = selectors.DefaultSelector()
    buffers = {p.stdout: bytearray(), p.stderr: bytearray()}
    for stream in buffers:
        os.set_blocking(stream.fileno(), False)
        sel.register(stream, selectors.EVENT_READ)
    end = time.monotonic() + deadline
    failure = None
    try:
        while sel.get_map() or p.poll() is None:
            if time.monotonic() >= end:
                failure = 'timeout'; break
            for key, _ in sel.select(min(.1, max(0, end-time.monotonic()))):
                data = os.read(key.fileobj.fileno(), 4096)
                if not data:
                    sel.unregister(key.fileobj); continue
                buffers[key.fileobj].extend(data)
                if len(buffers[key.fileobj]) > LIMIT:
                    failure = 'output_limit'; break
            if failure: break
        if failure:
            # Only our unreaped direct child/group. bwrap --die-with-parent and
            # its PID namespace own the inner tree; no ambient PID is targeted.
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
            p.wait(timeout=2)
        else: p.wait(timeout=2)
        return p.returncode, bytes(buffers[p.stdout][:LIMIT]), failure
    finally:
        if p.poll() is None:
            p.kill(); p.wait(timeout=2)
        sel.close()
        p.stdout.close(); p.stderr.close()


def pin(path):
    fd = os.open(path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC)
    md = os.fstat(fd)
    if not (stat.S_ISDIR(md.st_mode) or (stat.S_ISREG(md.st_mode) and md.st_nlink==1)):
        os.close(fd); raise OSError(errno.EINVAL, 'mount_identity')
    return fd


def composed(root, fds):
    # filesystem.rs Boundary::append_to at PIN: same namespace/cap/environment
    # flags, mount order, O_PATH sources, destinations, and denials last.
    argv = ['/usr/bin/bwrap','--unshare-all','--unshare-user','--unshare-cgroup',
            '--die-with-parent','--cap-drop','ALL','--new-session','--clearenv',
            '--tmpfs','/','--dir','/lane']
    mounts=[]
    for path in ('/usr','/bin','/lib','/lib64'):
        if os.path.exists(path): mounts.append((os.path.realpath(path),path,False))
    for name in ('workspace','store','cache','state'):
        mounts.append((root+'/'+name,'/lane/'+name,True))
    mounts.append((root+'/outside','/outside',False))
    for name in ('.aida','.git','.agents','.codex','.claude','AGENTS.md','CLAUDE.md','.mcp.json'):
        mounts.append((root+'/workspace/'+name,'/lane/workspace/'+name,False))
    for source,destination,writable in mounts:
        fd=pin(source); fds.append(fd)
        argv.extend(['--bind-fd' if writable else '--ro-bind-fd',str(fd),destination])
    argv.extend(['--dev','/dev','--proc','/proc','--tmpfs','/tmp','--tmpfs','/run',
                 '--setenv','HOME','/lane/state','--setenv','PATH','/usr/bin:/bin',
                 '--chdir','/lane/workspace','--remount-ro','/'])
    return argv


def main():
    if sys.argv[1:] not in (['--host'], ['--nested-proxy']):
        print('usage: /usr/bin/python3 -I -S SCRIPT --host|--nested-proxy',file=sys.stderr)
        return 64
    if os.geteuid()==0 or os.getuid()!=os.geteuid():
        print(json.dumps({'stage':'require_ordinary_unprivileged_identity','errno':errno.EPERM}))
        return 1
    # --host is an operator assertion of execution location, not auto-attestation.
    context = 'HOST_OPERATOR_ASSERTED' if sys.argv[1]=='--host' else 'NESTEDPROXY'
    os.umask(0o077)
    root = tempfile.mkdtemp(prefix='aida-task2705-precondition-',dir='/tmp')
    parent = os.open('/tmp',os.O_RDONLY|os.O_DIRECTORY|os.O_CLOEXEC)
    rootfd = os.open(root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
    identity = os.fstat(rootfd)
    retain = False
    result = {'version':1,'source_pin':PIN,'context':context,
              'acceptance_credit':False,'kernel_release':os.uname().release,
              'phases':[]}
    try:
        for name in ('workspace','store','cache','state','control'):
            os.mkdir(root+'/'+name,0o700)
        for name in ('.aida','.git','.agents','.codex','.claude'):
            os.mkdir(root+'/workspace/'+name,0o700)
            with open(root+'/workspace/'+name+'/existing','x') as f: f.write('protected')
        for name in ('AGENTS.md','CLAUDE.md','.mcp.json'):
            with open(root+'/workspace/'+name,'x') as f: f.write('protected')
        with open(root+'/outside','x') as f: f.write('outside-control')
        # Before confinement the intended negative-control target really IS writable.
        with open(root+'/workspace/.aida/existing','w') as f: f.write('protected')
        result['host_owned_write_positive']=True
        env={'HOME':root+'/state','PATH':'/usr/bin:/bin','TMPDIR':root,
             'XDG_CONFIG_HOME':root+'/state','XDG_CACHE_HOME':root+'/cache',
             'XDG_STATE_HOME':root+'/state','XDG_RUNTIME_DIR':root+'/state',
             'LANG':'C','LC_ALL':'C'}
        rc,out,fail=bounded(['/usr/bin/bwrap','--version'],env,deadline=2)
        text=out.decode('ascii','replace').strip()
        result['bwrap_version']=text if rc==0 and not fail and re.fullmatch(r'bubblewrap [0-9.]+',text) else 'unavailable'
        for place in ('outside','composed'):
            for variant in ('combined-raw','combined-mapped','split-mapped'):
                fds=[]
                try:
                    argv=composed(root,fds) if place=='composed' else []
                    # Same typed info-fd option as supervisor; /dev/null sink
                    # is a diagnostic delta (we do not claim reaper acceptance).
                    if place=='composed':
                        info=os.open('/dev/null',os.O_WRONLY|os.O_CLOEXEC); fds.append(info)
                        argv[1:1]=['--info-fd',str(info)]
                        argv.append('--')
                    # Outside target is host-owned scratch, never shared /tmp.
                    # Composed raw branch retains the production mountpoint.
                    control='/tmp/nested-control' if place=='composed' else root+'/control'
                    argv.extend(['/usr/bin/python3','-I','-S','-c',WORKER,place,variant,control])
                    rc,out,fail=bounded(argv,env,fds)
                    events=[]
                    try:
                        for line in out.decode('utf-8').splitlines():
                            event=json.loads(line)
                            if not isinstance(event,dict) or 'stage' not in event: raise ValueError()
                            events.append(event)
                    except (ValueError,UnicodeError): fail='invalid_channel'; events=[]
                    phase={'place':place,'variant':variant,'returncode':rc,'events':events,
                           'transport_failure':fail,'complete':rc==0 and not fail and
                           bool(events) and events[-1].get('stage')=='phase_complete'}
                    if not events: phase['unreached']='worker_entry; no nested-control credit'
                    result['phases'].append(phase)
                    if fail in ('timeout','output_limit'):
                        retain=True
                        break
                finally:
                    for fd in fds: os.close(fd)
            if retain: break
        result['host_controls_unchanged']=(open(root+'/workspace/.aida/existing').read()=='protected'
                                           and open(root+'/outside').read()=='outside-control')
    except OSError as e:
        result['controller_refusal']={'stage':'setup_or_cleanup','errno':e.errno}
        retain=True
    except BaseException:
        # A bounded-wait failure/interruption must never trigger optimistic
        # resource deletion or publish exception payloads from a child.
        result['controller_refusal']={'stage':'controller_interrupted_or_unsettled','errno':None}
        retain=True
    finally:
        try:
            md=os.stat(os.path.basename(root),dir_fd=parent,follow_symlinks=False)
            same=(md.st_dev,md.st_ino,md.st_uid)==(identity.st_dev,identity.st_ino,os.getuid())
        except OSError:
            same=False
        if same and not retain and shutil.rmtree.avoids_symlink_attacks:
            try:
                shutil.rmtree(os.path.basename(root),dir_fd=parent)
                result['temp_cleanup']='exact_owned_root_removed'
            except OSError as e:
                result['temp_cleanup']='refused; no fallback'
                result['cleanup_errno']=e.errno
                result['owned_temp_root']=root
        else:
            result['temp_cleanup']='retained; no host unmount or recursive fallback'
            result['owned_temp_root']=root
        os.close(rootfd); os.close(parent)
    print(json.dumps(result,sort_keys=True,indent=2))
    good=(len(result['phases'])==6 and all(p['complete'] for p in result['phases'])
          and result.get('host_controls_unchanged') and result.get('temp_cleanup')=='exact_owned_root_removed')
    return 0 if good else 1


if __name__=='__main__':
    sys.exit(main())
