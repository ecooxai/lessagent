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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading

class Fixture(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        self.send_response(200); self.send_header('Content-Type','application/json'); self.end_headers()
        self.wfile.write(json.dumps({'models':[{'slug':'gpt-5.4-mini','display_name':'Fixture'}]}).encode())
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert self.headers['Authorization']=='Bearer fixture-token'
        assert body['stream'] is True and body['store'] is False
        is_light = not body.get('tools') and 'Lessagent Light mode' in body.get('instructions','')
        assert body['reasoning']['effort']==('medium' if is_light or body.get('tools') else 'low')
        prompt=json.dumps(body)
        n=len(list(data.glob('prompt-*.txt')))
        (data/f'prompt-{n}.txt').write_text(prompt)
        if 'CANCEL_FIXTURE' in prompt: time.sleep(2)
        outputs=[item for item in body['input'] if item.get('type')=='function_call_output']
        if is_light and 'LIGHT_WEBAPP_FIXTURE' in prompt:
            import re
            light_step=1+sum('LIGHT_WEBAPP_FIXTURE' in p.read_text() for p in data.glob('prompt-*.txt') if p != data/f'prompt-{n}.txt')
            assert '<bash>' in body['instructions'] and '<python>' in body['instructions']
            assert 'LESSAGENT_OUTPUT_DIR' in body['instructions'] and 'LESSAGENT_LOG_DIR' in body['instructions']
            assert not outputs
            assert all(item.get('type') not in ('function_call','function_call_output') for item in body['input'])
            assert 'hello.txt' in prompt
            assert 'agent/context' not in prompt and 'agent/msgs' not in prompt and 'agent/output-history' not in prompt and 'agent/continuity' not in prompt
            if light_step > 1:
                # The image written during the previous iteration is attached
                # to this request for visual review, alongside the project image.
                assert 'preview.png' in prompt
                assert sum(c.get('type') == 'input_image' for item in body['input'] for c in item.get('content', [])) >= 2
            if light_step == 1:
                text='''<review>Starting a small web app.</review>
<summary>Build the page, save evidence, and review the generated preview next.</summary>
<scores>5</scores>
<plan>Create index.html and verify it.</plan>
<bash>cat > index.html <<'EOF'
<!doctype html><html><body><h1>Light app</h1></body></html>
EOF
cat index.html > "$LESSAGENT_LOG_DIR/build.log" 2>&1</bash>
<bash>cp pixel.png "$LESSAGENT_OUTPUT_DIR/preview.png"</bash>
<done>false</done>'''
            else:
                text='''<review>Implemented and verified the app.</review>
<scores>10</scores>
<plan>Finished.</plan>
<bash>test -s index.html && grep -q 'Light app' index.html > "$LESSAGENT_LOG_DIR/verify.log" 2>&1</bash>
<done>true</done>'''
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':text}]}]
        elif not body.get('tools') and 'FAIL_COMPACT' in prompt:
            self.send_response(500); self.end_headers(); self.wfile.write(b'Fixture compaction unavailable'); return
        elif not body.get('tools') and 'LESSAGENT_QUALITY_REVIEW' in prompt:
            review='Missing assessment' if 'BAD_REVIEW_FIXTURE' in prompt else f'<score>{9 if "BOUNDARY_REVIEW_FIXTURE" in prompt else 10}</score>Task complete based on the supplied action results.'
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':review}]}]
        elif not body.get('tools'):
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'COMPACT_SUMMARY: Current task COMPACT_FIXTURE. Completed three iterations. Latest tests passed. Next verify completion. Avoid repeating earlier mistakes.'}]}]
        elif 'COMPACT_FIXTURE' in prompt:
            import re
            step=int(re.findall(r'Iteration (\d+)\.',prompt)[-1])
            text=f'REPLY_STEP_{step} <score>{10 if step == 4 else 8}</score><test>printf compact-evidence; cp pixel.png "$LESSAGENT_OUTPUT_DIR/proof.png"</test>'
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':text}]}]
        elif 'SCORE_NINE_FIXTURE' in prompt:
            output=[{'type':'function_call','call_id':'nine','name':'shell','arguments':json.dumps({'command':'printf verified > outcome.txt','wait_ms':1000})}, {'type':'message','role':'assistant','content':[{'type':'output_text','text':'**Done** <score>9</score><test>test -s outcome.txt</test>'}]}]
        elif 'NO_TAGS_FIXTURE' in prompt and 'observed-no-tags' in prompt:
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'Implemented the task.<test>test -s outcome.txt</test>'}]}]
        elif 'NO_TAGS_FIXTURE' in prompt:
            output=[{'type':'function_call','call_id':'no-tags','name':'shell','arguments':json.dumps({'command':'printf observed-no-tags > outcome.txt; cat outcome.txt','wait_ms':1000})}]
        elif 'JOKE_FIXTURE' in prompt:
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'Because light attracts bugs.'}]}]
        elif 'FIRST_FIXTURE' in prompt:
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'PRIOR_CHAT_SENTINEL <score>10</score><done><done>'}]}]
        elif not outputs and 'written' not in prompt:
            output=[{'type':'function_call','call_id':'write','name':'write_file','arguments':json.dumps({'path':'result.txt','text':'updated fixture content'})}]
        elif len(outputs)==1 or 'tool_probe_done' not in prompt:
            output=[{'type':'function_call','call_id':'shell','name':'shell','arguments':json.dumps({'command':'test -d "$LESSAGENT_OUTPUT_DIR" && printf env-ready > "$LESSAGENT_OUTPUT_DIR/shell-env.txt"; cat result.txt; printf tool_probe_done','wait_ms':1000})}]
        else:
            output=[{'type':'message','role':'assistant','content':[{'type':'output_text','text':'Fixture task complete <score>10</score>'}]}]
        if 'NO_TAGS_FIXTURE' not in prompt and 'SCORE_NINE_FIXTURE' not in prompt and any(item['type']=='function_call' for item in output):
            output.append({'type':'message','role':'assistant','content':[{'type':'output_text','text':"<score>8</score><test>test \"$(cat result.txt)\" = 'updated fixture content' && printf evidence > \"$LESSAGENT_OUTPUT_DIR/evidence.txt\"</test>"}]})
        event={'type':'response.completed','response':{'output':output,'usage':{'input_tokens':100,'input_tokens_details':{'cached_tokens':30},'output_tokens':20}}}
        self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.end_headers()
        try:
            payload=('data: '+json.dumps(event)+'\n\n').encode()
            for i in range(0,len(payload),17): self.wfile.write(payload[i:i+17]); self.wfile.flush()
        except (BrokenPipeError,ConnectionResetError): pass

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
    fixture=ThreadingHTTPServer(('127.0.0.1',0),Fixture)
    threading.Thread(target=fixture.serve_forever,daemon=True).start()
    auth=root/'auth'; auth.mkdir()
    (auth/'auth.json').write_text(json.dumps({'tokens':{'access_token':'fixture-token','account_id':'fixture-account'}}))
    env = {k:v for k,v in os.environ.items() if k not in ('OPENAI_API_KEY','ANTHROPIC_API_KEY','GEMINI_API_KEY')}
    env['CODEX_HOME']=str(auth)
    env['LESSAGENT_CODEX_BASE_URL']=f'http://127.0.0.1:{fixture.server_port}'
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
        assert {'terminals','history','git'} <= {t['kind'] for t in w['ui']['tabs']}
        act('workspace_close',workspace=wid)
        assert next(w for w in request('/api/state')['workspaces'] if w['id']==wid)['closed']
        assert not act('workspace_open',path=str(project))['closed']
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
        models=request('/api/models/codex'); assert models[0]['id']=='gpt-5.4-mini'
        assert completed(act('run',workspace=wid,prompt='FIRST_FIXTURE')['job_id'])['status']=='completed'
        # Remove the first task's saved knowledge so we can distinguish history from summaries.
        for p in (project/'agent/knowledge/done').glob('*.md'): p.unlink()
        import shutil
        shutil.rmtree(project/'agent/output/chat')
        act('workspace_mode',workspace=wid,mode='light')
        light_begin=len(list(data.glob('prompt-*.txt')))
        light=completed(act('run',workspace=wid,prompt='LIGHT_WEBAPP_FIXTURE: build and test a small web app using hello.txt')['job_id'])
        assert light['status']=='completed',light
        assert (project/'index.html').read_text().find('Light app') >= 0
        assert light['usage']=={'input_tokens':140,'cached_input_tokens':60,'output_tokens':40},light
        assert light['elapsed_ms']>0
        assert light['score']==10 and light['thinking']=='medium'
        light_prompts=sorted(data.glob('prompt-*.txt'))[light_begin:]
        assert len(light_prompts)==2
        light_bodies=[json.loads(p.read_text()) for p in light_prompts]
        # One request has the instruction, task, and context turns; the next
        # adds only the first bounded assistant reply.
        assert [len(b['input']) for b in light_bodies]==[3,4]
        assert all(not b.get('tools') for b in light_bodies)
        assert all(not any(item.get('type') in ('function_call','function_call_output') for item in b['input']) for b in light_bodies)
        assert 'Starting a small web app' in json.dumps(light_bodies[1])
        assert 'hello.txt' in json.dumps(light_bodies[0])
        assert all('agent/context' not in p.read_text() and 'agent/msgs' not in p.read_text() and 'agent/output-history' not in p.read_text() for p in light_prompts)
        assert all('additional context' not in p.read_text().lower() for p in light_prompts)
        assert any('agent/output/' in p.read_text() for p in light_prompts[1:])
        light_requests=[e for e in light['events'] if e['kind']=='request']
        assert [len(e['recent_ai_messages']) for e in light_requests]==[0,1]
        assert all(e['context_policy']=='files_and_recent_ai' for e in light_requests)
        assert all(e['part_tokens']['context']>0 for e in light_requests)
        assert all(e['project_files'] for e in light_requests)
        assert any(e['kind']=='light_action' and e['language']=='bash' for e in light['events'])
        light_results=[e['result'] for e in light['events'] if e['kind']=='light_result' and e.get('language')=='bash']
        assert len(light_results)==3 and all(r['exit_code']==0 and r['meaningful'] for r in light_results)
        current_files=list((project/'agent/output').rglob('*'))
        archived_files=list((project/'agent/output-history').rglob('*'))
        continuity_files=list((project/'agent/continuity').rglob('*'))
        assert (project/'agent/continuity/chat/summary.md').exists()
        assert any(p.name=='state.json' for p in continuity_files)
        assert any(p.name=='actions.json' for p in continuity_files)
        assert any(p.name=='result.json' for p in continuity_files)
        assert any(p.name=='build.log' for p in continuity_files)
        assert any(p.name=='verify.log' for p in continuity_files)
        assert any(p.name=='preview.png' for p in archived_files + current_files)
        assert any('Build the page, save evidence' in p.read_text(errors='ignore') for p in continuity_files if p.name == 'actions.json')
        bookkeeping={'actions.json','result.json','state.json','summary.md','draw.log','bash.log'}
        assert not any(p.is_file() and (p.name in bookkeeping or p.suffix.lower()=='.log') for p in current_files)
        assert not any(p.is_file() and (p.name in bookkeeping or p.suffix.lower()=='.log') for p in archived_files)
        assert archived_files
        assert not any('echo test pass' in p.read_text(errors='ignore').lower() for p in archived_files + current_files if p.is_file())
        assert not list(data.glob('codex-image-*'))
        assert list((project/'agent/knowledge/done').glob('*.md'))
        assert len([e for e in light['events'] if e['kind']=='usage'])==2
        assert not (project/'agent/context/chat/project.txt').exists()
        act('workspace_mode',workspace=wid,mode='normal')
        jid=act('run',workspace=wid,prompt='CANCEL_FIXTURE')['job_id']
        cancel_prompt_count=len(list(data.glob('prompt-*.txt')))
        wait_for(lambda:len(list(data.glob('prompt-*.txt')))==cancel_prompt_count+1)
        act('stop',job_id=jid); assert completed(jid)['status']=='cancelled'
        wait_for(lambda:not list(data.glob('codex-image-*')))
        begin=len(list(data.glob('prompt-*.txt')))
        compact_job=completed(act('run',workspace=wid,session='agent-test',prompt='COMPACT_FIXTURE')['job_id'])
        assert compact_job['status']=='completed',compact_job
        assert compact_job['usage']=={'input_tokens':350,'cached_input_tokens':150,'output_tokens':100},compact_job
        snapshots=[json.loads((data/f'prompt-{i}.txt').read_text()) for i in range(begin,begin+5)]
        assert all('hello project' not in json.dumps(p) and 'PRIOR_CHAT_SENTINEL' not in json.dumps(p) for p in snapshots)
        assert any(c.get('type')=='input_image' for p in snapshots[1]['input'] for c in p.get('content',[]))
        assert any(c.get('type')=='input_image' for p in snapshots[3]['input'] for c in p.get('content',[]))
        assert all(snapshots[i]['input'][:2]==snapshots[0]['input'][:2] for i in [1,2,4])
        assert len([e for e in compact_job['events'] if e['kind']=='usage'])==5
        assert snapshots[3]['model']=='gpt-5.6-luna' and not snapshots[3].get('tools')
        assert 'REPLY_STEP_1' in json.dumps(snapshots[3]) and 'compact-evidence' in json.dumps(snapshots[3])
        assert 'COMPACT_SUMMARY' in json.dumps(snapshots[4]) and 'REPLY_STEP_1' not in json.dumps(snapshots[4])
        archive=project/'agent/msgs'/compact_job['id']/'iteration-3'
        assert len(list(archive.glob('message-*.json')))==3
        assert (project/'agent/context/agent-test/summary.md').exists()
        assert len(list((project/'agent/context/agent-test').glob('message-*.json')))==1
        assert all(e['project_files'] for e in compact_job['events'] if e['kind']=='request')
        act('settings',context_ignores=[])
        inv=request('/api/inventory/'+wid)
        assert not any(f['path'].startswith(('agent/context/','agent/msgs/')) for f in inv['files'])
        assert any(f['path'].startswith('agent/output/') for f in inv['files'])
        act('draft',workspace=wid,session='agent-test',text='isolated draft')
        assert request('/api/state')['workspaces'][0]['ui']['drafts']['agent-test']=='isolated draft'
        failure_job=completed(act('run',workspace=wid,session='agent-failure',prompt='COMPACT_FIXTURE FAIL_COMPACT')['job_id'])
        assert failure_job['status']=='completed' and failure_job['usage_incomplete'],failure_job
        assert len(list((project/'agent/context/agent-failure').glob('message-*.json')))==4
        assert not (project/'agent/msgs'/failure_job['id']).exists()
        assert any(e['kind']=='compaction' and e['status']=='failed' for e in failure_job['events'])
        joke=completed(act('run',workspace=wid,session='agent-joke',prompt='JOKE_FIXTURE tell me a joke')['job_id'])
        assert joke['status']=='completed'
        assert len([e for e in joke['events'] if e['kind']=='model'])==1
        assert not any(e['kind'] in ('tool','test') for e in joke['events'])
        recovered=completed(act('run',workspace=wid,session='agent-no-tags',prompt='NO_TAGS_FIXTURE')['job_id'])
        assert recovered['status']=='completed',recovered
        assert recovered['score']==10
        assert len([e for e in recovered['events'] if e['kind']=='model'])==2
        tests=[e['result'] for e in recovered['events'] if e['kind']=='test']
        assert len(tests)==1 and tests[0]['command']=='test -s outcome.txt' and tests[0]['exit_code']==0
        assert recovered['usage']=={'input_tokens':210,'cached_input_tokens':90,'output_tokens':60}
        for session, prompt in [('agent-boundary', 'SCORE_NINE_FIXTURE'), ('agent-boundary-review', 'NO_TAGS_FIXTURE BOUNDARY_REVIEW_FIXTURE')]:
            boundary=completed(act('run',workspace=wid,session=session,prompt=prompt)['job_id'])
            assert boundary['status']=='completed',boundary
            assert boundary['score']==9,boundary
            assert len([e for e in boundary['events'] if e['kind']=='model'])==(2 if 'NO_TAGS' in prompt else 1)
            assert any(e['kind']=='text' for e in boundary['events'])
        bad=completed(act('run',workspace=wid,session='agent-bad-review',prompt='NO_TAGS_FIXTURE BAD_REVIEW_FIXTURE')['job_id'])
        assert bad['status']=='failed' and 'Stopped instead of repeating unscored actions' in bad['output']
        assert len([e for e in bad['events'] if e['kind']=='model'])==2
        act('settings',provider='openai',model='gpt-5.4-mini')
        failed=completed(act('run',workspace=wid,prompt='Missing key')['job_id'])
        assert failed['status']=='failed' and 'OPENAI_API_KEY' in failed['output']
        act('workspace_mode',workspace=wid,mode='light')
        (project/'large.txt').write_text('x '*30000)
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
        info_req=urllib.request.Request(base+'/mcp', headers={
            'Origin':'https://service-check.example',
            'Sec-Fetch-Site':'cross-site',
        })
        with urllib.request.urlopen(info_req, timeout=5) as info_response:
            info_body=info_response.read().decode()
            assert info_response.headers.get('Access-Control-Allow-Origin')=='*'
        assert info_body.splitlines()[0]=='OK'
        assert '<summary><code>shell</code></summary>' in info_body and 'main_task' in info_body and 'current_timestamp' in info_body and 'wait_n' in info_body
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
