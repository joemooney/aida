# trace:TASK-1607 | ai:codex
# Hermetic actual-CLI takeover regressions; only fixture-owned children are targets.
import datetime, json, os, pathlib, subprocess, sys, tempfile, time, uuid, textwrap


def main():
    BIN = sys.argv[1]
    MODE = sys.argv[2] if len(sys.argv)>2 else 'expired'
    ACTION = sys.argv[3] if len(sys.argv)>3 else 'ack'
    worker = '''
    import json,subprocess,sys
    for line in sys.stdin:
        args=json.loads(line)
        r=subprocess.run([sys.argv[1],*args],capture_output=True,text=True,timeout=20)
        print(json.dumps(dict(code=r.returncode,out=r.stdout,err=r.stderr)),flush=True)
    '''
    worker = textwrap.dedent(worker)
    assert ACTION in ("ack", "release")
    with tempfile.TemporaryDirectory(prefix='c7-lifecycle-') as tmp:
        root=pathlib.Path(tmp); home=root/'home'; repo=root/'repo'
        home.mkdir(); repo.mkdir()
        env={k:v for k,v in os.environ.items() if not k.startswith(('AIDA_','CLAUDE','CODEX','GIT_'))}
        env.update(HOME=str(home), AIDA_USER='c7-fixture', GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null')
        subprocess.run(['git','init','-q','-b','main',str(repo)],env=env,check=True)
        (repo/'.aida').mkdir(); (repo/'.aida/config.toml').write_text('store_path = ".aida-store"\n')
        (repo/'.aida-store/objects').mkdir(parents=True)
        (repo/'.aida-store/registry').mkdir()
        (repo/'.aida-store/registry/team.toml').write_text('[members]\nholder = ["orchestrator"]\nrequester = ["orchestrator"]\n')
        grants=home/'.aida/session-grants'; grants.mkdir(parents=True)
        def mint(subject):
            now=datetime.datetime.now(datetime.timezone.utc); gid=str(uuid.uuid4())
            data=dict(id=gid,principal=subject,subject=subject,session_id=str(uuid.uuid4()),seat='orchestrator',tty_issued_at=now.isoformat(),delegable_seats=[],parent_grant_id=None,issued_at=now.isoformat(),expires_at=(now+datetime.timedelta(hours=1)).isoformat(),revoked_at=None)
            (grants/(gid+'.json')).write_text(json.dumps(data)); return gid,data
        script=root/'worker.py'; script.write_text(worker)
        workers=[]
        def start(grant, subject):
            p=subprocess.Popen(['codex',str(script),BIN],executable=sys.executable,cwd=repo,env={**env,'AIDA_SESSION_GRANT':grant,'AIDA_USER':subject},stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
            workers.append(p); return p
        def call(p,*args):
            p.stdin.write(json.dumps(args)+'\n'); p.stdin.flush()
            result=json.loads(p.stdout.readline()); print(json.dumps(dict(args=args,**result)),flush=True); return result
        try:
            ga,da=mint('holder'); gb,db=mint('requester'); a=start(ga,'holder'); b=start(gb,'requester')
            assert call(a,'role','current')['out'].strip()=='orchestrator'
            assert call(a,'session','seat','claim')['code']==0
            assert call(b,'session','seat','takeover','--force','--grace-secs',('1' if MODE in ('expired','interrupted') else '120'),'--no-wait')['code']==0
            status=call(a,'session','seat','status','--json'); rec=json.loads(status['out']); request=rec['request']['id']; generation=rec['generation']
            assert rec['holder']['anchor_pid']==a.pid, (rec,a.pid)
            assert rec['request']['requester_anchor']['pid']==b.pid, (rec,b.pid)
            if MODE in ('expired','interrupted'):
                if MODE=='interrupted':
                    b.stdin.close(); b.wait(timeout=5)
                time.sleep(1.2)
            elif MODE != 'control':
                grant_path=grants/(gb+'.json')
                if MODE=='revoked': db['revoked_at']=datetime.datetime.now(datetime.timezone.utc).isoformat()
                elif MODE=='grant-expired': db['expires_at']=datetime.datetime.now(datetime.timezone.utc).isoformat()
                elif MODE=='subject': db['subject']='different-subject'
                elif MODE=='session': db['session_id']=str(uuid.uuid4())
                elif MODE=='roster': (repo/'.aida-store/registry/team.toml').write_text('[members]\nholder = ["orchestrator"]\n')
                elif MODE=='roster-missing': (repo/'.aida-store/registry/team.toml').unlink()
                elif MODE=='roster-corrupt': (repo/'.aida-store/registry/team.toml').write_text('invalid [')
                elif MODE=='missing': grant_path.unlink()
                elif MODE=='corrupt': grant_path.write_text('{broken')
                else: raise AssertionError('unknown mode '+MODE)
                if MODE in ('revoked','grant-expired','subject','session'): grant_path.write_text(json.dumps(db))
                if MODE != 'session':
                    assert call(b,'role','current')['out'].strip()=='', 'requester grant must actually be invalid'

            ack=call(a,'session','seat',ACTION,'--generation',str(generation),'--request',request,*(['--safe-to-stop'] if ACTION=='ack' else []))
            after=json.loads(call(a,'session','seat','status','--json')['out'])
            print(MODE.upper()+'_ACK_TRANSFERRED='+str(ack['code']==0 and after['holder']['session']==db['session_id']),flush=True)
            if MODE=='control':
                assert ack['code']==0 and after['holder']['session']==db['session_id']
                print('PASS: valid unexpired requester transfers normally',flush=True)
            else:
                assert ack['code']!=0, f'BUG: actual CLI {ACTION} transferred to {MODE} request/requester'
                assert after['holder']['session']==da['session_id'] and after['generation']==generation
                if MODE not in ('roster-missing','roster-corrupt'):
                    assert after['request'] is None, after
                print('PASS: refused without transfer/demotion',flush=True)
        finally:
            for p in workers:
                if not p.stdin.closed: p.stdin.close()
            for p in workers:
                try: p.wait(timeout=5)
                except subprocess.TimeoutExpired: p.kill(); p.wait()


if __name__ == "__main__":
    main()
