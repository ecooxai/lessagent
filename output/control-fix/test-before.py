#!/usr/bin/env python3
"""macOS integration test. Starts isolated Chrome + Lessagent; native input only.
Requires Chrome, Accessibility and Screen Recording. Restores the frontmost app
following Chrome launch. Does not use WebDriver/CDP/JS to inject input or draw.
Usage: uv run --with 'mcp>=1.20,<2' tests/computer_background.py target/release/lessagent
Optional --allow-foreground records focus changes without requiring an unfocused target.
Optional --desktop-scroll also tests global-pointer scroll against the fixture.
"""
from contextlib import ExitStack
from anyio.from_thread import start_blocking_portal
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

import base64, collections, json, math, pathlib, signal, socket, struct, subprocess, sys, tempfile, threading, time
import urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARY = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/lessagent').resolve()
ALLOW_FOREGROUND = '--allow-foreground' in sys.argv
DESKTOP_SCROLL = '--desktop-scroll' in sys.argv
OUTPUT = ROOT / 'output/background-control'
OUTPUT.mkdir(parents=True, exist_ok=True)
records = []
class Drawing(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        self.send_response(200); self.send_header('Content-Type', 'text/html'); self.end_headers()
        self.wfile.write((ROOT/'tests/fixtures/drawing.html').read_bytes())
    def do_POST(self):
        records.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
        self.send_response(200); self.end_headers()

assert sys.platform == 'darwin', 'This integration test requires macOS'
with tempfile.TemporaryDirectory(prefix='lessagent-background-test-') as directory:
    root = pathlib.Path(directory)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]
    drawing = ThreadingHTTPServer(('127.0.0.1', 0), Drawing)
    drawing_port = drawing.server_address[1]
    threading.Thread(target=drawing.serve_forever, daemon=True).start()
    log = (root/'server.log').open('w')
    server = subprocess.Popen([str(BINARY), 'serve', '--port', str(port), '--data-dir', str(root/'data')], stdout=log, stderr=log)
    before = None
    clients = ExitStack()
    chrome_pid = None
    try:
        def request(path, body=None):
            data = None if body is None else json.dumps(body).encode()
            headers={'Content-Type':'application/json'}
            if path=='/mcp': headers['Authorization']='Bearer '+(root/'data/token').read_text().strip()
            req = urllib.request.Request(f'http://127.0.0.1:{port}'+path, data=data, headers=headers)
            with urllib.request.urlopen(req, timeout=30) as response: return json.load(response)
        def act(name, body): return request('/api/action/'+name, body)
        for _ in range(300):
            try: state = request('/api/state'); break
            except OSError: time.sleep(.05)
        else: raise AssertionError('Test server failed to start')
        settings = state['settings']; settings['computer_enabled'] = True
        act('settings', settings)
        portal = clients.enter_context(start_blocking_portal())
        streams = clients.enter_context(portal.wrap_async_context_manager(streamablehttp_client(
            f'http://127.0.0.1:{port}/mcp', headers={'Authorization':'Bearer '+(root/'data/token').read_text().strip()})))
        session = clients.enter_context(portal.wrap_async_context_manager(ClientSession(streams[0], streams[1])))
        portal.call(session.initialize)
        workspace = act('workspace_open', {'path':str(ROOT)})['id']
        def computer(**args): return act('tool', {'workspace':workspace, 'name':'computer', 'arguments':args})
        initial = computer(action='windows')
        assert initial['accessibility'] and initial['screen_recording'], 'Enable macOS Accessibility and Screen Recording'
        before = initial['desktop']
        subprocess.run(['open','-gj','-n','-a','Google Chrome','--args',f'--user-data-dir={root / "chrome"}',
                        '--no-first-run','--no-default-browser-check','--disable-background-networking',
                        '--window-size=1000,750',f'--app=http://127.0.0.1:{drawing_port}/'],check=True)
        for _ in range(80):
            windows = computer(action='windows')['windows']
            target = next((w for w in windows if w['title'] == f'Lessagent Background Drawing Test {drawing_port}'),None)
            if target: break
            time.sleep(.1)
        else: raise AssertionError('Chrome fixture window did not appear')
        chrome_pid = target['pid']
        # Chrome's first launch can activate itself despite open -g. Restore once
        # before beginning the actual background-input test.
        # Activating the previous bundle is ambiguous when it was Chrome: it
        # can activate our new isolated Chrome process instead. Use Finder as
        # a deterministic foreground app and restore the original PID at exit.
        subprocess.run(['osascript','-e','tell application "Finder" to activate'],check=True,capture_output=True)
        time.sleep(.5)
        def send(**args): return computer(window_id=target['window_id'],pid=target['pid'],**args)
        shot = send(action='screenshot',capture_path='output/background-control/before.png')
        assert shot['logical_width']==1000 and shot['logical_height']==750
        width,height = shot['screen_width'],shot['screen_height']
        responses=[]
        def input_action(**args):
            # Some Chrome versions activate asynchronously after a background
            # click. Establish an unfocused target for each delivery check.
            started=time.monotonic()
            result=portal.call(session.call_tool, 'computer', dict(workspace=workspace,
                window_id=target['window_id'],pid=chrome_pid,**args)).model_dump(by_alias=True)
            assert not result['isError'], result
            r=json.loads(next(c['text'] for c in result['content'] if c['type']=='text'))
            image=next(c for c in result['content'] if c['type']=='image')
            assert image['mimeType']=='image/png'
            r['image']={'data':image['data']}
            assert time.monotonic()-started >= 2, 'Observation returned before settling delay'
            assert r['automatic_screenshot'] and r['observation_delay_ms']==2000
            assert base64.b64decode(r['image']['data']) == pathlib.Path(r['path']).read_bytes()
            assert (r['screen_width'],r['screen_height']) == (width,height)
            r.pop('image') # Keep binary payloads out of diagnostic logs.
            r['foreground_after_observation'] = computer(action='windows')['desktop']['frontmost_pid']
            responses.append(r)
            if not ALLOW_FOREGROUND:
                assert r['before']['frontmost_pid'] != chrome_pid, 'Chrome became foreground before input'
            if not ALLOW_FOREGROUND:
                assert r['before']['frontmost_pid']==r['after']['frontmost_pid'], 'Input stole foreground focus'
            return r
        # Exercise both logical-point input and Retina screenshot-pixel input.
        input_action(action='click',x=220,y=97)
        input_action(action='type',text='Background starX')
        input_action(action='key',key='shift+left')
        input_action(action='type',text='!')
        time.sleep(.2)
        texts=[e['text'] for e in records if e['type']=='input']
        assert texts and texts[-1]=='Background star!', texts
        # Invalid endpoints and wrong ownership must not post partial input.
        n=len(records)
        for invalid in [dict(action='drag',path=[[500,220],[-20,200]]),
                        dict(action='drag',x=500,y=220),dict(action='click',x=-10,y=200),
                        dict(action='click',x=500,y=300,pid=1)]:
            try: computer(window_id=target['window_id'],**({'pid':chrome_pid}|invalid))
            except urllib.error.HTTPError as e: assert e.code==400
            else: raise AssertionError('Invalid request accepted')
        time.sleep(.1); assert len(records)==n, 'Invalid request produced input'
        path=[[round((500+220*math.sin(i*4*math.pi/5))*width/1000),
               round((440-220*math.cos(i*4*math.pi/5))*height/750)] for i in range(6)]
        shot=input_action(action='drag',path=path,screen_width=width,screen_height=height,duration=3,capture_path='output/background-control/star.png')
        assert shot['pointer_pressed_color']=='#2563EB'
        time.sleep(.2)
        pointers=[e for e in records if e['type'].startswith('pointer')]
        counts=collections.Counter(e['type'] for e in pointers)
        assert counts['pointerdown']==1 and counts['pointerup']==1 and counts['pointermove']>100, counts
        assert all(e['trusted'] for e in records if 'trusted' in e), 'Input was untrusted'
        if not ALLOW_FOREGROUND:
            assert all(not e['focused'] for e in records if 'focused' in e), 'Chrome had focus'
        for x,y in path:
            # Canvas starts 132 logical points below the window top (32 title + 100 header).
            assert min(math.hypot(e['x']-x*1000/width,e['y']-(y*750/height-132)) for e in pointers)<5
        assert shot['pointer_overlay'], 'Screenshot is missing virtual pointer overlay'
        assert struct.unpack('>II',(OUTPUT/'star.png').read_bytes()[16:24])==(width,height), 'Pointer overlay changed screenshot resolution'
        input_action(action='scroll',x=800,y=500,delta=3)
        time.sleep(.2)
        assert any(e['type']=='wheel' and e['delta']>0 for e in records), 'Background scroll did not reach Chrome'
        for distance in [10, -10, 0]:
            start=len(records)
            input_action(action='scroll',x=800,y=500,distance=distance,
                         screen_width=1000,screen_height=750)
            wheels=[e for e in records[start:] if e['type']=='wheel']
            if distance:
                assert wheels and all(e['delta']*distance < 0 for e in wheels), wheels
                assert all(abs(e['x']-800)<3 and abs(e['y']-468)<3 for e in wheels), wheels
            else:
                assert not any(e['delta'] for e in wheels), wheels
        unchanged=sum(r['before']['cursor_x']==r['after']['cursor_x'] and r['before']['cursor_y']==r['after']['cursor_y'] for r in responses)
        assert unchanged, 'No stationary-cursor sample; rerun while the physical mouse is idle'
        desktop_results=[]
        if DESKTOP_SCROLL:
            subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of (first application process whose unix id is {chrome_pid}) to true'],check=True,capture_output=True)
            time.sleep(.5)
            desktop=computer(action='windows')['desktop']
            assert desktop['frontmost_pid']==chrome_pid, 'Cannot establish foreground desktop fixture'
            target=next(w for w in computer(action='windows')['windows'] if w['window_id']==target['window_id'])
            for distance in [10,-10]:
                start=len(records)
                result=portal.call(session.call_tool,'computer',dict(workspace=workspace,
                    action='scroll',mode='desktop',x=round(target['x']+800),y=round(target['y']+500),distance=distance))
                assert not result.isError, result
                assert any(c.type=='image' and c.mimeType=='image/png' for c in result.content)
                wheels=[e for e in records[start:] if e['type']=='wheel']
                assert wheels and all(e['delta']*distance<0 for e in wheels), wheels
                assert all(abs(e['x']-800)<3 and abs(e['y']-468)<3 for e in wheels), wheels
                desktop_results.append({'distance':distance,'events':wheels})
            # Return the shared pointer to its starting position.
            computer(action='move',mode='desktop',x=round(before['cursor_x']),y=round(before['cursor_y']))
        report={'passed':True,'browser':'Google Chrome','input_delivery':'native macOS events; no browser automation',
                'desktop_scroll':desktop_results,'events':dict(counts),'text':texts[-1],'all_events_trusted':True,'all_events_unfocused':all(not e['focused'] for e in records if 'focused' in e),
                'foreground_preserved_during_delivery':all(r['before']['frontmost_pid']==r['after']['frontmost_pid'] for r in responses),'foreground_preserved_after_observation':all(r['foreground_after_observation']!=chrome_pid for r in responses),'stationary_cursor_actions':unchanged,'actions':len(responses),
                'screenshots':['before.png','star.png'],'results':responses,'recorded_events':records}
        (OUTPUT/'report.json').write_text(json.dumps(report,indent=2))
        print(json.dumps({k:v for k,v in report.items() if k not in ('results','recorded_events')},indent=2))
    finally:
        clients.close()
        if chrome_pid:
            try: subprocess.run(['kill','-TERM',str(chrome_pid)],check=False,capture_output=True)
            except OSError: pass
        if before and before.get('frontmost_pid'):
            subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of first application process whose unix id is {before["frontmost_pid"]} to true'],capture_output=True)
        server.send_signal(signal.SIGINT)
        try: server.wait(timeout=10)
        except subprocess.TimeoutExpired: server.kill(); server.wait()
        drawing.shutdown(); drawing.server_close(); log.close()
