#!/usr/bin/env python3
"""Explicit prompt CLI output regression; isolated Git store and HOME.

Run: python3 tests/test_origin_prompt.py --aida target/agent/aida
trace:STORY-1468 | ai:codex
"""
import argparse
import json
import os
import re
import tempfile
from pathlib import Path
from test_cache_refresh_outputs import run, require
from test_show_latency import wait_for_fixture_processes

parser = argparse.ArgumentParser()
parser.add_argument("--aida", required=True)
binary = str(Path(parser.parse_args().aida).resolve())
sentinel = 'ORIGIN_PROMPT_SMOKE_SENTINEL'
with tempfile.TemporaryDirectory(prefix='aida-origin-prompt-smoke-') as temp:
    root, home = Path(temp)/'project', Path(temp)/'home'
    root.mkdir(); home.mkdir()
    env = {k:v for k,v in os.environ.items() if not k.startswith(('AIDA_', 'GIT_'))}
    env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home/'.config'),
        XDG_CACHE_HOME=str(home/'.cache'), XDG_DATA_HOME=str(home/'.local/share'),
        XDG_STATE_HOME=str(home/'.local/state'),
        GIT_AUTHOR_NAME='Test', GIT_AUTHOR_EMAIL='test@example.invalid',
        GIT_COMMITTER_NAME='Test', GIT_COMMITTER_EMAIL='test@example.invalid',
        AIDA_TELEMETRY='0', AIDA_CACHE_READ_WAIT_MS='20', AIDA_CACHE_RETRY_COUNT='0')
    def cli(*args):
        result = run([binary,*args], root, env, timeout=25)
        require(result)
        return result.stdout
    def added_id(output):
        match = re.search(r'Added:\s+([A-Z]+-[A-Za-z0-9-]+)', output)
        assert match, output
        return match.group(1)
    try:
        require(run(['git','-c','maintenance.auto=false','-c','gc.auto=0','init','-q'], root, env))
        cli('init','--no-skills','--no-hooks')
        prompt = root/'reviewed.txt'; prompt.write_text(sentinel+' [REDACTED]\n')
        spec = added_id(cli('add','--title','Normalized smoke feature','--type','story',
            '--description','Normalized contract','--origin-prompt-file',str(prompt),'--origin-prompt-redacted'))
        for flags in [('--json','--no-git'),('--card','--full','--no-git'),('--full','--no-git')]:
            assert sentinel not in cli('show',spec,*flags)
        assert sentinel not in cli('list','--format','json')
        assert 'Normalized smoke feature' not in cli('search',sentinel)
        text = cli('show',spec,'--origin-prompt','--format','human')
        assert sentinel in text and 'untrusted historical user input' in text and 'Redacted' in text
        document = json.loads(cli('show', spec, '--origin-prompt', '--format', 'json'))
        assert document['origin_prompt']['text'] == sentinel+' [REDACTED]\n'
        assert document == json.loads(cli('show', spec, '--origin-prompt', '--json'))
        assert document['origin_prompt']['disposition'] == 'redacted'
        assert 'not current instructions' in document['interpretation']
        cli('edit',spec,'--description','Edited normalized contract')
        assert sentinel in cli('show',spec,'--origin-prompt')
        absent = added_id(cli('add','--title','No prompt capture','--type','task','--no-origin-prompt'))
        assert 'No originating prompt captured' in cli('show',absent,'--origin-prompt')
        absent_json = json.loads(cli('show', absent, '--origin-prompt', '--format', 'json'))
        assert absent_json['origin_prompt'] is None
        prompt.write_text('password=known-synthetic-secret-value')
        refused = run([binary,'add','--title','Refused smoke feature','--type','task',
            '--origin-prompt-file',str(prompt)], root, env, timeout=25)
        assert refused.returncode != 0
        assert 'known-synthetic-secret-value' not in refused.stdout+refused.stderr
        assert 'Refused smoke feature' not in cli('list','--format','json')
        print('PASS: CLI captured/redacted/absent/refused intake, explicit retrieval, edit preservation, and default JSON/card/full/list/search omission', flush=True)
    finally:
        wait_for_fixture_processes(temp, timeout=25)
