#!/usr/bin/env python3
"""Debug-only regression for original-profile reuse, with no personal browser data.
Run: uv run --with 'mcp>=1.20,<2' tests/browser_profile_safety.py target/debug/lessagent
All Chrome profiles, endpoints, and website records in this test are disposable.
"""
from contextlib import ExitStack
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from anyio.from_thread import start_blocking_portal
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client
import base64
import json
import os
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
REPORT = ROOT / 'output/profile-safety-validation'
TASK = 'Verify original Chrome profile data and first-profile selection'
HTML = b'''<!doctype html><meta charset="utf-8"><title>Chrome profile safety fixture</title>
<style>body{font:18px system-ui;padding:30px}input{font:inherit;width:450px;padding:10px}pre{white-space:pre-wrap}</style>
<h1>Original profile persistence test</h1><p>Local test data only. No account credentials.</p>
<input id="name" aria-label="Profile persistence marker"><pre id="report"></pre>
<script>
(async()=>{
 const params=new URLSearchParams(location.search),seed=params.get('seed'),tag=params.get('tag');
 const key='lessagent-profile-safety';
 const db=await new Promise((resolve,reject)=>{const r=indexedDB.open(key,1);r.onupgradeneeded=()=>r.result.createObjectStore('state');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
 if(seed){localStorage.setItem(key,seed);document.cookie='lessagent_profile_probe='+seed+';path=/;max-age=86400;SameSite=Lax';await new Promise((resolve,reject)=>{const t=db.transaction('state','readwrite');t.objectStore('state').put(seed,'marker');t.oncomplete=resolve;t.onerror=()=>reject(t.error)})}
 const idb=await new Promise((resolve,reject)=>{const r=db.transaction('state').objectStore('state').get('marker');r.onsuccess=()=>resolve(r.result??null);r.onerror=()=>reject(r.error)});
 const field=document.querySelector('#name');field.value=localStorage.getItem(key)||'';
 const box=field.getBoundingClientRect();
 const value={type:'ready',tag,local:localStorage.getItem(key),cookie:document.cookie.split('; ').find(c=>c.startsWith('lessagent_profile_probe='))||null,idb,innerHeight,innerWidth,box:{x:box.x,y:box.y,width:box.width,height:box.height}};
 document.querySelector('#report').textContent=JSON.stringify(value,null,2);
 const post=v=>fetch('/event',{method:'POST',body:JSON.stringify(v)});
 await post(value);
 for(const type of ['pointermove','pointerdown','pointerup','click']) field.addEventListener(type,e=>post({type,tag,trusted:e.isTrusted,button:e.button,buttons:e.buttons}));
 field.addEventListener('input',e=>{localStorage.setItem(key,field.value);post({type:'input',tag,value:field.value,trusted:e.isTrusted})});
})().catch(e=>fetch('/event',{method:'POST',body:JSON.stringify({type:'error',message:String(e)})}));
</script>'''


def wait_for(fn, message, seconds=20):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = fn()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError(message)


def exercise():
    assert sys.platform == 'darwin'
    assert 'debug' in BINARY.parts, 'Use a debug binary only'
    REPORT.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'binary': str(BINARY), 'checks': []}
    events = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_GET(self):
            self.send_response(200); self.send_header('Content-Type','text/html; charset=utf-8'); self.end_headers(); self.wfile.write(HTML)
        def do_POST(self):
            value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            events.append(value); self.send_response(204); self.end_headers()

    page = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=page.serve_forever, daemon=True).start()
    base_page = f'http://127.0.0.1:{page.server_port}/'

    with tempfile.TemporaryDirectory(prefix='lessagent-profile-safety-') as directory:
        temp = Path(directory); data = temp/'data'; source = temp/'original-chrome'
        for name in ['Default', 'Profile 2', 'Profile 10']:
            folder = source/name; folder.mkdir(parents=True)
            (folder/'Preferences').write_text(json.dumps({'profile': {'name': 'Synthetic '+name}}))
            (folder/'PreservationMarker').write_text('test data: '+name)
        (source/'Local State').write_text(json.dumps({'profile': {'profiles_order':['Default','Profile 2','Profile 10'], 'last_used':'Profile 10'}}))
        legacy = data/'browsers'/'existing-Default'; legacy.mkdir(parents=True)
        (legacy/'DoNotDelete').write_text('old copy must remain untouched')
        with socket.socket() as sock:
            sock.bind(('127.0.0.1',0)); port = sock.getsockname()[1]
        assert port != 3210
        log = (REPORT/'server.log').open('w')
        server = subprocess.Popen(
            [str(BINARY),'serve','--port',str(port),'--data-dir',str(data)],
            stdout=log, stderr=log,
            env={**os.environ,'LESSAGENT_CHROME_PROFILE_SOURCE':str(source)},
        )
        clients = ExitStack(); chrome_pids = set()

        def launch_profile(user_data, profile, url, debugging=True, left=80, top=80):
            args=['open','-g','-n','-a','Google Chrome','--args',f'--user-data-dir={user_data}']
            if profile:
                args.append(f'--profile-directory={profile}')
            args += ['--new-window','--no-first-run','--no-default-browser-check','--window-size=1000,600',f'--window-position={left},{top}']
            if debugging:
                args += ['--remote-debugging-address=127.0.0.1','--remote-debugging-port=0']
            subprocess.run(args+[url],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)

        def ready(tag):
            return next((e for e in reversed(events) if e.get('type')=='ready' and e.get('tag')==tag),None)

        def api(path, body=None):
            req=urllib.request.Request(
                f'http://127.0.0.1:{port}'+path,
                data=None if body is None else json.dumps(body).encode(),
                headers={'Content-Type':'application/json'},
            )
            with urllib.request.urlopen(req,timeout=45) as response:
                return json.load(response)

        def alive():
            try: return api('/api/state')
            except OSError: return None

        try:
            state = wait_for(alive,'Debug backend not ready')
            state['settings']['computer_enabled']=True
            api('/api/action/settings',state['settings'])
            workspace=api('/api/action/workspace_open',{'path':str(ROOT)})['id']
            portal=clients.enter_context(start_blocking_portal())
            streams=clients.enter_context(portal.wrap_async_context_manager(streamablehttp_client(f'http://127.0.0.1:{port}/mcp')))
            client=clients.enter_context(portal.wrap_async_context_manager(ClientSession(streams[0],streams[1])))
            portal.call(client.initialize)
            guide=portal.call(client.read_resource,'lessagent://server/instruction.md').contents[0].text
            assert '--profile-directory' in guide and 'profiles_order' in guide and 'browser_open` tool' in guide
            definitions={tool.name for tool in portal.call(client.list_tools).tools}
            assert 'browser_open' not in definitions and {'bash','python','list_windows','get_screenshot','virtual_pointer','virtual_keyboard'} <= definitions

            calls=[]
            def call(name, **args):
                if name in ('virtual_pointer','virtual_keyboard','get_screenshot'):
                    args['capture_path']=str((REPORT/f'capture-{len(calls):02d}.png').relative_to(ROOT))
                result=portal.call(client.call_tool,name,dict(
                    workspace=workspace,
                    summary=(f'Ready; {name}')[:500],
                    agent='profile-safety-regression', model='test-client', main_task=TASK,
                    current_task=f'Exercise shell Chrome {name}', progress=70, quality=95,
                    current_timestamp=datetime.now().astimezone().isoformat(), **args,
                ))
                if result.isError:
                    text='\n'.join(c.text for c in result.content if c.type=='text')
                    raise AssertionError(text)
                value=result.structuredContent['result']
                calls.append({'name':name,'result':{k:value[k] for k in ('pid','window_id','path','capture_backend','delivery','before','after') if k in value}})
                if name in ('virtual_pointer','virtual_keyboard','get_screenshot'):
                    images=[c for c in result.content if c.type=='image']; assert len(images)==1
                    raw=base64.b64decode(images[0].data)
                    assert struct.unpack('>II',raw[16:24])==(value['width'],value['height'])
                    assert raw==Path(value['path']).read_bytes()
                return value

            def windows(): return call('list_windows')['windows']

            def open_shell_window(user_data, profile, tag, seed=None, debugging=True, left=80, top=80):
                before={w['window_id'] for w in windows()}
                query=f'?tag={tag}&purpose=shell_profile_safety_by_test_client'
                if seed is not None: query += '&seed='+seed
                launch_profile(user_data,profile,base_page+query,debugging,left,top)
                event=wait_for(lambda:ready(tag),f'{tag} page did not load')
                last_candidates=[]
                def created():
                    nonlocal last_candidates
                    candidates=[w for w in windows() if w.get('app')=='Google Chrome' and w['window_id'] not in before]
                    last_candidates=candidates
                    titled=[w for w in candidates if 'Chrome profile safety fixture' in (w.get('title') or '')]
                    if len(titled)==1:
                        return titled[0]
                    fitted=[w for w in candidates if abs(float(w.get('x',-9999))-left)<=12 and abs(float(w.get('y',-9999))-top)<=12
                            and abs(float(w.get('width',0))-1000)<=12 and abs(float(w.get('height',0))-600)<=12]
                    if len(fitted)==1:
                        return fitted[0]
                    return candidates[0] if len(candidates)==1 else None
                try:
                    window=wait_for(created,f'Could not uniquely identify shell-opened Chrome window for {tag}')
                except AssertionError:
                    report['window_discovery_failure']={'tag':tag,'before':sorted(before),'candidates':last_candidates}
                    raise
                chrome_pids.add(window['pid'])
                return window,event

            first_window,first_ready=open_shell_window(source,'Default','first','first-original',True,80,80)
            first=call('get_screenshot',window_id=first_window['window_id'],pid=first_window['pid'],mode='background')
            assert first['capture_backend']=='browser-surface-window-frame' and not first['browser_chrome_captured']
            assert first_ready['local']=='first-original' and first_ready['cookie']=='lessagent_profile_probe=first-original' and first_ready['idb']=='first-original'
            report['checks'].append('Shell-opened original Default profile auto-adopted through approved DevTools without browser_open')

            box=first_ready['box']; top=first['logical_height']-first_ready['innerHeight']
            x=round((box['x']+box['width']/2)*first['width']/first['logical_width'])
            y=round((top+box['y']+box['height']/2)*first['height']/first['logical_height'])
            target={'window_id':first_window['window_id'],'pid':first_window['pid'],'mode':'background'}
            actions=[
                ('virtual_pointer',dict(action='click',x=x,y=y,screen_width=first['width'],screen_height=first['height'])),
                ('virtual_keyboard',dict(action='key',key='cmd+a')),
                ('virtual_keyboard',dict(action='type',text='persisted by shell adoption')),
            ]
            for name,args in actions:
                value=call(name,**target,**args)
                assert value['delivery']=='chrome-devtools'
                assert value['before']['frontmost_pid']!=first_window['pid'] and value['after']['frontmost_pid']!=first_window['pid']
            wait_for(lambda:any(e.get('type')=='input' and e.get('trusted') and e.get('value')=='persisted by shell adoption' for e in events),'Trusted background input was not received')
            report['checks'].append('Adopted Chrome accepts trusted background pointer/keyboard input and stays out of foreground')

            second_window,second_ready=open_shell_window(source,'Default','second',None,True,160,120)
            second=call('get_screenshot',window_id=second_window['window_id'],pid=second_window['pid'],mode='background')
            if second['capture_backend']!='browser-surface-window-frame':
                debug={'window':second_window,'capture':{k:second.get(k) for k in ['capture_backend','geometry_source','logical_width','logical_height']}}
                try:
                    debug_port=(source/'DevToolsActivePort').read_text().splitlines()[0]
                    with urllib.request.urlopen(f'http://127.0.0.1:{debug_port}/json/list',timeout=3) as response:
                        debug['targets']=[{'id':t.get('id'),'title':t.get('title'),'url':t.get('url'),'type':t.get('type')} for t in json.load(response)]
                except Exception as error:
                    debug['target_error']=repr(error)
                report['second_adoption_debug']=debug
                print(json.dumps(debug,indent=2),flush=True)
            assert second['capture_backend']=='browser-surface-window-frame'
            assert second_ready['local']=='persisted by shell adoption' and second_ready['idb']=='first-original'
            report['checks'].append('Second shell-opened Default window preserves original profile storage')

            fresh=temp/'fresh-chrome'
            fresh_window,fresh_ready=open_shell_window(fresh,None,'fresh',None,True,240,160)
            fresh_shot=call('get_screenshot',window_id=fresh_window['window_id'],pid=fresh_window['pid'],mode='background')
            assert fresh_shot['capture_backend']=='browser-surface-window-frame'
            assert fresh_ready['local'] is None and fresh_ready['cookie'] is None and fresh_ready['idb'] is None
            report['checks'].append('Explicit disposable --user-data-dir is independently discovered/adopted and starts blank')

            readonly=temp/'readonly-chrome'
            readonly_window,readonly_ready=open_shell_window(readonly,None,'readonly',None,False,320,200)
            readonly_shot=call('get_screenshot',window_id=readonly_window['window_id'],pid=readonly_window['pid'],mode='background')
            assert readonly_shot['capture_backend']!='browser-surface-window-frame'
            box=readonly_ready['box']; readonly_top=readonly_shot['logical_height']-readonly_ready['innerHeight']
            readonly_x=round(box['x']+box['width']/2); readonly_y=round(readonly_top+box['y']+box['height']/2)
            assert readonly_window.get('background_input') == 'chrome-devtools-or-native-window', readonly_window
            native_start=max((e.get('seq',0) for e in events),default=0)
            native_target=dict(window_id=readonly_window['window_id'],pid=readonly_window['pid'],mode='background')
            native_results=[
                call('virtual_pointer',**native_target,action='move',x=readonly_x,y=readonly_y),
                call('virtual_pointer',**native_target,action='click',x=readonly_x,y=readonly_y),
                call('virtual_pointer',**native_target,action='drag',x=readonly_x-20,y=readonly_y+10,to_x=readonly_x+40,to_y=readonly_y+50,duration=.2),
                call('virtual_keyboard',**native_target,action='type',text='native-bg'),
            ]
            for value in native_results:
                assert value['delivery']=='skylight-process-window',value
                assert value['target_was_background'] is True,value
                assert value['background_focus_without_raise'] is True,value
                assert value['before']['frontmost_pid']!=readonly_window['pid'] and value['after']['frontmost_pid']!=readonly_window['pid'],value
            time.sleep(.2)
            # Background fetch logging can be throttled for this fixture, so verify
            # recipient state through the profile itself: click focused the input,
            # authenticated background typing changed it, and its input handler persisted
            # that value to localStorage. A second normal window must observe it.
            readonly_check_window,readonly_check_ready=open_shell_window(
                readonly,None,'readonly-check',None,False,360,240)
            assert readonly_check_ready['local']=='native-bg',readonly_check_ready
            assert readonly_check_window['pid']==readonly_window['pid'],(readonly_window,readonly_check_window)
            report['checks'].append('Fully background Chrome without Remote Debugging accepts SkyLight pointer/drag/type delivery and persists recipient state')

            assert (legacy/'DoNotDelete').read_text()=='old copy must remain untouched'
            for name in ['Default','Profile 2','Profile 10']:
                assert (source/name/'PreservationMarker').read_text()=='test data: '+name
            assert not (legacy/'Default').exists(), 'Original profile was cloned into old Lessagent copy'
            report['checks'].append('Original fixture profiles and legacy copy remain untouched')
            report.update(passed=True,port=port,calls=calls,first_profile='Default')
            print(json.dumps({k:v for k,v in report.items() if k!='calls'},indent=2),flush=True)
        finally:
            clients.close()
            try:
                rows=subprocess.check_output(['ps','-ax','-o','pid=','-o','command='],text=True).splitlines()
            except Exception:
                rows=[]
            for row in rows:
                pid,_,cmd=row.strip().partition(' ')
                if cmd.startswith('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome ') and f'--user-data-dir={temp}' in cmd:
                    chrome_pids.add(int(pid))
            for pid in chrome_pids:
                try: os.kill(pid,signal.SIGTERM)
                except ProcessLookupError: pass
            time.sleep(1)
            if server.poll() is None:
                server.send_signal(signal.SIGINT)
                try: server.wait(timeout=10)
                except subprocess.TimeoutExpired: server.terminate(); server.wait(timeout=10)
            log.close(); page.shutdown(); page.server_close()
            (REPORT/'report.json').write_text(json.dumps(report,indent=2))


if __name__ == '__main__':
    exercise()
