#!/usr/bin/env python3
"""Strict macOS background-control regression, through MCP HTTP, MCP stdio and
normal HTTP tool execution. Input is native only: no CDP/WebDriver/JS injection.
The fixture's JS observes events and paints in response to genuine native input.
Usage: uv run --with 'mcp>=1.20,<2' tests/computer_background.py [binary]
Requires Chrome, Accessibility and Screen Recording. Only disposable test
windows/data are changed. The physical pointer is never deliberately moved.
"""
from contextlib import ExitStack
from anyio.from_thread import start_blocking_portal
from mcp import ClientSession, StdioServerParameters
from mcp.client.streamable_http import streamablehttp_client
from mcp.client.stdio import stdio_client
import base64, collections, json, math, pathlib, signal, socket, struct
import subprocess, sys, tempfile, threading, time, traceback
import urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path('/Users/ecoo/project/agent/lessagent')
BINARY = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/lessagent').resolve()
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

def line_distance(p, a, b):
    dx,dy=b[0]-a[0],b[1]-a[1]
    t=max(0,min(1,((p[0]-a[0])*dx+(p[1]-a[1])*dy)/(dx*dx+dy*dy))) if dx or dy else 0
    return math.hypot(p[0]-a[0]-t*dx,p[1]-a[1]-t*dy)

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
    before = None; chrome_pid = None; clients = ExitStack(); responses = []; checks = []
    report = {'passed': False, 'binary': str(BINARY), 'input_delivery': 'native process/window events only'}
    try:
        def request(path, body=None):
            data = None if body is None else json.dumps(body).encode()
            req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=data,headers={'Content-Type':'application/json'})
            with urllib.request.urlopen(req, timeout=45) as response: return json.load(response)
        def act(name, body): return request('/api/action/'+name, body)
        for _ in range(300):
            try: state=request('/api/state'); break
            except OSError: time.sleep(.05)
        else: raise AssertionError('Test backend did not start')
        settings=state['settings']; settings['computer_enabled']=True; act('settings',settings)
        workspace=act('workspace_open',{'path':str(ROOT)})['id']
        portal=clients.enter_context(start_blocking_portal())
        streams=clients.enter_context(portal.wrap_async_context_manager(streamablehttp_client(
            f'http://127.0.0.1:{port}/mcp',headers={'Authorization':'Bearer '+(root/'data/token').read_text().strip()})))
        http=clients.enter_context(portal.wrap_async_context_manager(ClientSession(streams[0],streams[1])))
        portal.call(http.initialize)
        streams2=clients.enter_context(portal.wrap_async_context_manager(stdio_client(StdioServerParameters(
            command=str(BINARY),args=['mcp','--port',str(port),'--data-dir',str(root/'data')]))))
        stdio=clients.enter_context(portal.wrap_async_context_manager(ClientSession(streams2[0],streams2[1])))
        portal.call(stdio.initialize)
        def mcp(args, transport='mcp-http'):
            return portal.call((stdio if transport=='mcp-stdio' else http).call_tool,'computer',dict(workspace=workspace,**args))
        def unpack(result):
            assert not result.isError,result
            return json.loads(next(c.text for c in result.content if c.type=='text'))
        def computer(**args): return act('tool',{'workspace':workspace,'name':'computer','arguments':args})
        initial=unpack(mcp({'action':'windows'}))
        assert initial['accessibility'] and initial['screen_recording'], 'Enable Accessibility and Screen Recording'
        before=initial['desktop']
        subprocess.run(['open','-gj','-n','-a','Google Chrome','--args',f'--user-data-dir={root/"chrome"}',
                        '--no-first-run','--no-default-browser-check','--disable-background-networking',
                        '--window-size=1000,750',f'--app=http://127.0.0.1:{drawing_port}/'],check=True)
        for _ in range(100):
            windows=computer(action='windows')['windows']
            target=next((w for w in windows if w['title']==f'Lessagent Background Drawing Test {drawing_port}'),None)
            if target and any(e['type']=='ready' for e in records): break
            time.sleep(.1)
        else: raise AssertionError('Chrome fixture did not become ready')
        chrome_pid=target['pid']
        # LaunchServices/Chrome may activate during first launch. Restore the
        # user's original process once, before measuring any control action.
        subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of first application process whose unix id is {before["frontmost_pid"]} to true'],check=True,capture_output=True)
        time.sleep(.6)
        target_args=dict(window_id=target['window_id'],pid=chrome_pid,mode='background')
        shot_result=mcp(dict(target_args,action='screenshot',show_pointer=False,capture_path='output/background-control/before.png'))
        shot=unpack(shot_result)
        assert any(c.type=='image' and c.mimeType=='image/png' for c in shot_result.content)
        width,height=shot['screen_width'],shot['screen_height']
        lw,lh=shot['logical_width'],shot['logical_height']
        ready=next(e for e in reversed(records) if e['type']=='ready')
        top=lh-ready['innerHeight']; geometry=ready['geometry']
        def at(element,fx=.5,fy=.5):
            r=geometry[element];return round(r['x']+r['width']*fx),round(top+r['y']+r['height']*fy)
        def newest():return max((e['seq'] for e in records),default=0)
        def since(seq):return sorted((e for e in records if e['seq']>seq),key=lambda e:e['seq'])
        def input_action(label,transport='mcp-http',**args):
            start=newest(); started=time.monotonic()
            if transport=='api':
                r=computer(**(target_args|args)); assert 'image' not in r,'HTTP must not embed image payloads'
            else:
                raw=mcp(target_args|args,transport); r=unpack(raw)
                image=next(c for c in raw.content if c.type=='image')
                assert image.mimeType=='image/png'
                assert base64.b64decode(image.data)==pathlib.Path(r['path']).read_bytes()
            assert time.monotonic()-started>=2,'Screenshot was returned before settling'
            assert r['automatic_screenshot'] and r['observation_delay_ms']==2000,r
            assert (r['screen_width'],r['screen_height'])==(width,height),r
            assert r['mode']=='background' and r['window_id']==target['window_id'] and r['pid']==chrome_pid,r
            # Document focus may be virtualized for the recipient's responder.
            # Actual OS foreground must never change to the background process,
            # including after the automatic-observation delay.
            for state_name in ['before','after','desktop']:
                assert r[state_name]['frontmost_pid']!=chrome_pid,(label,state_name,r)
            assert r.get('delivery')=='process-window',r
            r['label']=label;r['transport']=transport;responses.append(r)
            events=since(start)
            assert all(e.get('trusted',True) for e in events),'Untrusted synthesized DOM events'
            if args['action'] in ('click','move','drag','scroll'):
                assert all(not any(e.get(k,False) for k in ('meta','ctrl','alt','shift')) for e in events),'Unexpected mouse modifiers'
            print('PASS input',label,transport,flush=True)
            return events
        def check(name):checks.append(name);print('PASS check',name,flush=True)

        # Move and exact single-click behavior through all three entry points.
        for transport in ['mcp-http','mcp-stdio','api']:
            x,y=at('click-test');es=input_action('left click',transport,action='click',x=x,y=y)
            assert sum(e['type']=='button-click' for e in es)==1,es
            down=[e for e in es if e['type']=='pointerdown'];up=[e for e in es if e['type']=='pointerup']
            assert len(down)==len(up)==1 and down[0]['button']==0 and down[0]['buttons']==1 and up[0]['buttons']==0,es
        check('left click exactly once / normal API + both MCP transports')
        x,y=at('canvas',.7,.3)
        es=input_action('right click',action='click',button='right',x=x,y=y)
        assert sum(e['type']=='contextmenu' for e in es)==1,es
        assert any(e['type']=='pointerdown' and e['button']==2 and e['buttons']==2 for e in es),es
        assert any(e['type']=='pointerup' and e['button']==2 and e['buttons']==0 for e in es),es
        check('right click / correct button mask / release')

        x,y=at('name');input_action('focus input',action='click',x=x,y=y)
        es=input_action('Unicode type',action='type',text='Background cat 猫🐈X')
        assert [e['value'] for e in es if e['type']=='input'][-1]=='Background cat 猫🐈X',es
        input_action('shift+left',action='key',key='shift+left')
        es=input_action('replace selection',action='type',text='!')
        assert [e['value'] for e in es if e['type']=='input'][-1]=='Background cat 猫🐈!',es
        input_action('cmd+a',action='key',key='cmd+a')
        es=input_action('replace all',action='type',text='Cat controls ✓')
        assert [e['value'] for e in es if e['type']=='input'][-1]=='Cat controls ✓',es
        es=input_action('backspace',action='key',key='backspace')
        assert [e['value'] for e in es if e['type']=='input'][-1]=='Cat controls ',es
        es=input_action('tab',action='key',key='tab')
        assert any(e['type']=='keydown' and e['key']=='Tab' for e in es),es
        check('Unicode / emoji / text selection / modifier keys / backspace / Tab')

        x1,y1=at('range',.25);x2,y2=at('range',.9)
        es=input_action('slider drag Retina pixels',action='drag',x=round(x1*width/lw),y=round(y1*height/lh),to_x=round(x2*width/lw),to_y=round(y2*height/lh),screen_width=width,screen_height=height,duration=.5)
        values=[int(e['value']) for e in es if e['type']=='input' and e['target']=='range']
        assert values and values[-1]>=85,es
        check('native slider drag / Retina coordinate conversion')

        def stroke(label,path,**args):
            es=input_action(label,action='drag',**args)
            pointers=[e for e in es if e['type'].startswith('pointer') and e.get('target')=='canvas']
            down=[e for e in pointers if e['type']=='pointerdown'];up=[e for e in pointers if e['type']=='pointerup']
            assert len(down)==len(up)==1,(label,collections.Counter(e['type'] for e in pointers),pointers)
            assert down[0]['buttons']==1 and up[0]['buttons']==0,(label,pointers)
            active=[e for e in pointers if down[0]['seq']<e['seq']<up[0]['seq'] and e['type']=='pointermove']
            assert active and all(e['buttons']==1 for e in active),(label,active)
            assert not any(e['type']=='pointercancel' for e in pointers),(label,pointers)
            for e in active:
                point=(e['x'],e['y']+top)
                assert min(line_distance(point,a,b) for a,b in zip(path,path[1:]))<4,(label,'off-path movement',e)
            assert math.hypot(down[0]['x']-path[0][0],down[0]['y']+top-path[0][1])<3
            assert math.hypot(up[0]['x']-path[-1][0],up[0]['y']+top-path[-1][1])<3
            assert sum(e['type']=='stroke-complete' for e in es)==1,(label,es)
        stroke('horizontal shorthand',[(150,250),(700,250)],x=150,y=250,to_x=700,duration=.6)
        stroke('vertical shorthand',[(180,260),(180,500)],x=180,y=260,to_y=500,duration=.4)
        star=[[round(440+195*math.sin(i*4*math.pi/5)),round(435-195*math.cos(i*4*math.pi/5))] for i in range(6)]
        pixel_path=[[round(x*width/lw),round(y*height/lh)] for x,y in star]
        stroke('continuous star path',star,path=pixel_path,screen_width=width,screen_height=height,duration=3,capture_path='output/background-control/star.png')
        stroke('fast drag 50ms',[(200,650),(730,650)],start=[200,650],end=[730,650],duration_ms=50)
        for i in range(3):
            y=290+i*25;stroke('repeated drag '+str(i),[(210,y),(350,y)],x=210,y=y,to_x=350,to_y=y,duration=.1)
        check('horizontal / vertical / curved / fast / repeated continuous drags')
        assert struct.unpack('>II',(OUTPUT/'star.png').read_bytes()[16:24])==(width,height)
        es=input_action('right drag',action='drag',button='right',x=550,y=300,to_x=720,to_y=350,duration=.4)
        assert any(e['type']=='pointerdown' and e['button']==2 and e['buttons']==2 for e in es),es
        assert any(e['type']=='pointerup' and e['button']==2 and e['buttons']==0 for e in es),es
        es=input_action('move after drag',action='move',x=600,y=350)
        assert not any(e['type']=='pointermove' and e.get('buttons') for e in es),es
        check('right drag / no stuck button after release')

        x,y=at('scrollbox')
        es=input_action('scroll down',action='scroll',x=x,y=y,distance=-10)
        wheels=[e for e in es if e['type']=='wheel'];scrolls=[e['top'] for e in es if e['type']=='scrolled']
        assert wheels and all(e['delta']>0 for e in wheels) and scrolls and scrolls[-1]>0,es
        down_top=scrolls[-1]
        es=input_action('scroll up',action='scroll',x=x,y=y,distance=10)
        wheels=[e for e in es if e['type']=='wheel'];scrolls=[e['top'] for e in es if e['type']=='scrolled']
        assert wheels and all(e['delta']<0 for e in wheels) and scrolls and scrolls[-1]<down_top,es
        es=input_action('scroll zero',action='scroll',x=x,y=y,distance=0)
        assert not any(e['type']=='wheel' and e['delta'] for e in es),es
        es=input_action('legacy scroll',action='scroll',x=x,y=y,delta=3)
        wheels=[e for e in es if e['type']=='wheel']
        assert wheels and all(e['delta']>0 and abs(e['x']-x)<3 and abs(e['y']+top-y)<3 for e in wheels),es
        check('scroll directions / real scroll position / zero / legacy delta')

        # All invalid requests must fail before any responder or pointer event.
        bad=[{'action':'click','x':5,'y':5},
             dict(target_args,action='click',x=1,y=2,mode='bckground'),
             dict(target_args,action='click',x=1,y=2,mode='desktop'),
             dict(target_args,action='click',x=1,y=2,pid=1),
             dict(target_args,action='click',x=1,y=2,button='middle'),
             dict(target_args,action='click',x=-10,y=200),
             dict(target_args,action='drag',path=[[500,220],[-20,200]]),
             dict(target_args,action='drag',path=[[500,220],[99999,200]]),
             dict(target_args,action='drag',x=500,y=220),
             dict(target_args,action='drag',x=10,y=200,to_x=20,to_y=220,duration=-1),
             dict(target_args,action='drag',path=[[10,200],[20,220]],duration=True),
             dict(target_args,action='click',x=1,y=2,screen_width=1000),
             dict(target_args,action='click',x=1,y=2,screen_width=0,screen_height=750),
             dict(target_args,action='key',key='not-a-real-key'),
             dict(target_args,action='scroll',x=800,y=400,distance=10,delta=10)]
        start=newest()
        for args in bad:
            result=mcp(args);assert result.isError,('Invalid request accepted',args,result)
        time.sleep(.2)
        assert not since(start),('Invalid request produced input',since(start))
        check(str(len(bad))+' invalid requests / no partial input / no implicit desktop fallback')
        stable=sum(r['before']['cursor_x']==r['after']['cursor_x'] and r['before']['cursor_y']==r['after']['cursor_y'] for r in responses)
        assert stable>0,'No stationary-cursor sample was available'
        report.update(passed=True,browser='Google Chrome',checks=checks,actions=len(responses),stationary_cursor_actions=stable,
                      foreground_target_seen=False,foreground_preserved_after_observation=True,
                      virtual_document_focus=any(e.get('focused') for e in records),all_input_events_trusted=all(e.get('trusted',True) for e in records),
                      transports=sorted({r['transport'] for r in responses}),screenshots=['before.png','star.png'])
        print(json.dumps(report,indent=2),flush=True)
    except BaseException as error:
        report['failure']=str(error);report['traceback']=traceback.format_exc();raise
    finally:
        report['results']=responses;report['recorded_events']=sorted(records,key=lambda e:e.get('seq',0));report['checks']=checks
        (OUTPUT/'report.json').write_text(json.dumps(report,indent=2))
        clients.close()
        if chrome_pid:
            subprocess.run(['kill','-TERM',str(chrome_pid)],check=False,capture_output=True)
        # Do not reactivate an old app here: the user may have switched apps
        # while the test ran, and the background controls must respect that.
        server.send_signal(signal.SIGINT)
        try: server.wait(timeout=10)
        except subprocess.TimeoutExpired: server.kill();server.wait()
        drawing.shutdown();drawing.server_close();log.close()
        (OUTPUT/'server.log').write_text((root/'server.log').read_text())
