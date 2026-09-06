#!/usr/bin/env python3
"""Exercise the real server, CLI, PTYs and agent loop without API credentials."""
import base64
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import urllib.error

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
FAKE_CODEX = r'''#!/usr/bin/env python3
import json, pathlib, sys, time
if 'app-server' in sys.argv:
    for line in sys.stdin:
        r=json.loads(line)
        if 'id' not in r: continue
        result={} if r['method']=='initialize' else {'data':[{'model':'fixture-model','displayName':'Fixture'}],'nextCursor':None}
        print(json.dumps({'id':r['id'],'result':result}),flush=True)
    sys.exit()
prompt=sys.stdin.read()
output=pathlib.Path(sys.argv[sys.argv.index('--output-last-message')+1])
root=output.parent
n=len(list(root.glob('prompt-*.txt')))
(root/f'prompt-{n}.txt').write_text(prompt)
assert '--disable' in sys.argv and 'shell_tool' in sys.argv
if 'CANCEL_FIXTURE' in prompt: time.sleep(60)
if 'FIRST_FIXTURE' in prompt:
    text='PRIOR_CHAT_SENTINEL'
    calls=[]
elif not 'written' in prompt:
    text='Writing a result'
    calls=[{'name':'write_file','arguments':json.dumps({'path':'result.txt','text':'updated fixture content'})}]
elif not 'shell {"command":"cat result.txt"' in prompt:
    assert 'updated fixture content' in prompt
    text='Checking in a terminal'
    calls=[{'name':'shell','arguments':json.dumps({'command':'cat result.txt','wait_ms':1000})}]
else:
    text='Fixture task complete'
    calls=[]
output.write_text(json.dumps({'text':text,'calls':calls}))
'''

def wait_for(fn, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = fn()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError('Timed out waiting for condition')

with tempfile.TemporaryDirectory(prefix='lessagent-smoke-') as tmp:
    root = Path(tmp)
    data, project, fakebin = root/'data', root/'project', root/'bin'
    project.mkdir(); fakebin.mkdir()
    (project/'hello.txt').write_text('hello project')
    (project/'.gitignore').write_text('ignored.txt\n')
    (project/'ignored.txt').write_text('IGNORED_SENTINEL')
    (project/'pixel.png').write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jXioAAAAASUVORK5CYII='))
    (fakebin/'codex').write_text(FAKE_CODEX); (fakebin/'codex').chmod(0o755)
    env = {k:v for k,v in os.environ.items() if k not in ('OPENAI_API_KEY','ANTHROPIC_API_KEY','GEMINI_API_KEY')}
    env['PATH'] = str(fakebin)+os.pathsep+env['PATH']
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    args = [str(BINARY), '--port', str(port), '--data-dir', str(data)]
    server = None
    log = (root/'server.log').open('w+')
    def start():
        global server
        server = subprocess.Popen(args+['serve'], env=env, stdout=log, stderr=log)
        def healthy():
            if server.poll() is not None:
                log.seek(0); raise AssertionError(log.read())
            try: return urllib.request.urlopen(base+'/health', timeout=1).status == 200
            except (OSError, urllib.error.URLError): return False
        wait_for(healthy)
    def stop():
        if server and server.poll() is None:
            server.send_signal(signal.SIGINT)
            try: server.wait(timeout=10)
            except subprocess.TimeoutExpired: server.kill(); server.wait(); raise
    def request(path, body=None, headers=None, status=200):
        h = {'Authorization':'Bearer '+(data/'token').read_text()}
        if headers: h.update(headers)
        if body is not None: h['Content-Type']='application/json'
        req=urllib.request.Request(base+path, data=None if body is None else json.dumps(body).encode(), headers=h)
        try: response=urllib.request.urlopen(req, timeout=15)
        except urllib.error.HTTPError as e: response=e
        raw=response.read()
        assert response.status==status, (path,response.status,raw)
        return json.loads(raw) if raw and 'application/json' in response.headers.get('Content-Type','') else raw
    def act(action, **body): return request('/api/action/'+action,body)
    def completed(jid):
        return wait_for(lambda: next((j for j in request('/api/state')['jobs'] if j['id']==jid and j['status']!='running'),None))
    try:
        start()
        assert request('/api/state')['settings']['provider']=='codex'
        request('/api/state',headers={'Authorization':'Bearer invalid'},status=401)
        request('/api/state',headers={'Origin':'https://evil.example'},status=403)
        request('/api/state',headers={'Host':'lessagent.example','Origin':'http://lessagent.example'})
        request('/api/state',headers={'Host':'lessagent.example','Origin':'http://other.example'},status=403)
        second=subprocess.run(args+['serve'],capture_output=True,text=True,timeout=5)
        assert second.returncode and 'Another backend' in second.stderr
        w=act('workspace_open',path=str(project)); wid=w['id']
        assert act('workspace_open',path=str(project))['id']==wid
        inv=request('/api/inventory/'+wid)
        assert inv['light_allowed'] and inv['image_tokens']>0
        assert 'ignored.txt' not in [f['path'] for f in inv['files']]
        for path in ('../server.log',str(root/'server.log')):
            request('/api/action/tool',{'workspace':wid,'name':'read_file','arguments':{'path':path}},status=400)
        (project/'escape').symlink_to(root)
        request('/api/action/tool',{'workspace':wid,'name':'write_file','arguments':{'path':'escape/overwrite','text':'bad'}},status=400)
        (project/'escape').unlink()
        executable=project/'script.sh'; executable.write_text('old'); executable.chmod(0o755)
        act('tool',workspace=wid,name='write_file',arguments={'path':'script.sh','text':'new'})
        assert executable.stat().st_mode & 0o777 == 0o755
        models=request('/api/models/codex'); assert models[0]['id']=='fixture-model'
        assert completed(act('run',workspace=wid,prompt='FIRST_FIXTURE')['job_id'])['status']=='completed'
        # Remove the first task's saved knowledge so we can distinguish history from summaries.
        for p in (project/'agent/knowledge/done').glob('*.md'): p.unlink()
        act('workspace_mode',workspace=wid,mode='light')
        job=completed(act('run',workspace=wid,prompt='Edit and test the fixture')['job_id'])
        assert job['status']=='completed',job
        assert (project/'result.txt').read_text()=='updated fixture content'
        prompts=sorted(data.glob('prompt-*.txt'))[1:]
        assert len(prompts)==3
        assert all('PRIOR_CHAT_SENTINEL' not in p.read_text() and 'IGNORED_SENTINEL' not in p.read_text() for p in prompts)
        assert 'updated fixture content' in prompts[1].read_text()
        assert 'hello.txt' in (project/'agent/context/project.txt').read_text()
        assert list((project/'agent/knowledge/done').glob('*.md'))
        assert not list(data.glob('codex-image-*'))
        assert any(e['kind']=='tool' and e['name']=='shell' for e in job['events'])
        act('workspace_mode',workspace=wid,mode='normal')
        jid=act('run',workspace=wid,prompt='CANCEL_FIXTURE')['job_id']
        wait_for(lambda:len(list(data.glob('prompt-*.txt')))==5)
        act('stop',job_id=jid); assert completed(jid)['status']=='cancelled'
        wait_for(lambda:not list(data.glob('codex-image-*')))
        act('settings',provider='openai',model='fixture-model')
        failed=completed(act('run',workspace=wid,prompt='Missing key')['job_id'])
        assert failed['status']=='failed' and 'OPENAI_API_KEY' in failed['output']
        act('workspace_mode',workspace=wid,mode='light')
        (project/'large.txt').write_text('x'*30000)
        request('/api/action/workspace_mode',{'workspace':wid,'mode':'light'},status=400)
        (project/'large.txt').unlink()
        terminal=act('tool',workspace=wid,name='shell',arguments={'command':'sleep .3; printf backend-survived','wait_ms':0})
        tid=terminal['terminal_id']
        # No browser or polling is needed to keep the process alive.
        time.sleep(.6)
        output=act('tool',workspace=wid,name='terminal_read',arguments={'terminal_id':tid})
        assert 'backend-survived' in output['output'] and output['exited']
        live=act('tool',workspace=wid,name='shell',arguments={'command':'sleep 60','wait_ms':0})
        running=next(t for t in request('/api/state')['terminals'] if t['id']==live['terminal_id'])
        assert running['managed'] and running['running']
        act('tool',workspace=wid,name='terminal_stop',arguments={'terminal_id':live['terminal_id']})
        wait_for(lambda:act('tool',workspace=wid,name='terminal_read',arguments={'terminal_id':live['terminal_id']})['exited'])
        ui={'tabs':[{'id':'chat','kind':'chat','title':'Agent'},{'id':'terminals','kind':'terminals','title':'Terminals'}],'active':'terminals','draft':'persist me'}
        act('ui',workspace=wid,ui=ui);act('ui',ui={'selected':wid})
        act('draft',workspace=wid,text='updated draft')
        ui['draft']='updated draft'
        act('terminal_resize',terminal_id=tid,rows=30,cols=100,height=480)
        ui['terminal_heights']={tid:480}
        rpc={'jsonrpc':'2.0','id':1,'method':'tools/list'}
        assert len(request('/mcp',rpc)['result']['tools'])>=10
        request('/mcp',{'jsonrpc':'2.0','method':'notifications/initialized'},status=202)
        bridge=subprocess.run(args+['mcp'],input=json.dumps(rpc)+'\n',capture_output=True,text=True,timeout=10)
        assert bridge.returncode==0 and json.loads(bridge.stdout)['result']['tools']
        stop();start()
        state=request('/api/state')
        assert state['workspaces'][0]['ui']==ui and state['ui']['selected']==wid
        restored=next(t for t in state['terminals'] if t['id']==tid)
        assert restored['exited'] and 'backend-survived' in restored['screen']
        assert restored['managed'] and not restored['running']
        print('PASS: HTTP auth, file boundaries, model discovery, light context, agent tools, cancellation, PTYs, MCP, restart persistence')
    finally:
        stop();log.close()
