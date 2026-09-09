#!/usr/bin/env python3
"""Strict macOS background-control regression, through MCP HTTP, MCP stdio and
normal HTTP tool execution. Managed Chrome uses browser-local Input commands;
window capture and the independent overlay are native. No DOM events or canvas
operations are injected. The fixture only observes trusted browser input. Set
LESSAGENT_NATIVE_PROBE=1 to exercise the native process-window Chrome path.
Usage: uv run --with 'mcp>=1.20,<2' --with pillow tests/computer_background.py [binary]
Requires Chrome, Accessibility and Screen Recording. Only disposable test
windows/data are changed. The physical pointer is never deliberately moved.
"""
from contextlib import ExitStack
from anyio.from_thread import start_blocking_portal
from mcp import ClientSession, StdioServerParameters
from mcp.client.streamable_http import streamablehttp_client
from mcp.client.stdio import stdio_client
import base64, collections, json, math, pathlib, signal, socket, struct, os
import subprocess, sys, tempfile, threading, time, traceback
import urllib.request, urllib.error
from control_lab import LabServer
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARY = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
OUTPUT_REL = os.environ.get('LESSAGENT_CONTROL_REPORT_DIR','output/background-control')
OUTPUT = ROOT / OUTPUT_REL
OUTPUT.mkdir(parents=True, exist_ok=True)

def line_distance(p, a, b):
    dx,dy=b[0]-a[0],b[1]-a[1]
    t=max(0,min(1,((p[0]-a[0])*dx+(p[1]-a[1])*dy)/(dx*dx+dy*dy))) if dx or dy else 0
    return math.hypot(p[0]-a[0]-t*dx,p[1]-a[1]-t*dy)

assert sys.platform == 'darwin', 'This integration test requires macOS'
with tempfile.TemporaryDirectory(prefix='lessagent-background-test-', ignore_cleanup_errors=True) as directory:
    root = pathlib.Path(directory)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]
    drawing = LabServer(('127.0.0.1', 0))
    records = drawing.events
    drawing_port = drawing.server_address[1]
    threading.Thread(target=drawing.serve_forever, daemon=True).start()
    log = (root/'server.log').open('w')
    server = subprocess.Popen([str(BINARY), 'serve', '--port', str(port), '--data-dir', str(root/'data')], stdout=log, stderr=log)
    before = None; chrome_pid = None; clients = ExitStack(); responses = []; checks = []; accuracy = []; idle_samples = []
    native_probe=os.environ.get('LESSAGENT_NATIVE_PROBE')=='1'
    report = {'passed': False, 'binary': str(BINARY), 'input_delivery': 'native-window' if native_probe else 'chrome-devtools Input + page-surface window-relative observation; no DOM event injection'}
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
            return portal.call((stdio if transport=='mcp-stdio' else http).call_tool,'computer',dict(workspace=workspace,summary=f'Verify background computer {args.get("action", "observation")} through {transport}.',**args))
        def unpack(result):
            assert not result.isError,result
            # Prefer machine-readable content; presentation text may include
            # prose before an image and is not necessarily a JSON object.
            structured=getattr(result,'structuredContent',None)
            if isinstance(structured,dict) and isinstance(structured.get('result'),dict):
                return structured['result']
            for block in result.content:
                if block.type=='text':
                    try:return json.loads(block.text)
                    except (ValueError,TypeError):pass
            raise AssertionError('MCP result has no machine-readable metadata')
        def computer(**args): return act('tool',{'workspace':workspace,'name':'computer','arguments':args})
        initial=unpack(mcp({'action':'windows'}))
        assert initial['accessibility'] and initial['screen_recording'], 'Enable Accessibility and Screen Recording'
        before=initial['desktop']
        if native_probe:
            subprocess.run(['open','-gj','-n','-a','Google Chrome','--args',f'--user-data-dir={root/"chrome"}',
                            '--no-first-run','--no-default-browser-check','--disable-background-networking',
                            '--window-size=1000,750',f'--app=http://127.0.0.1:{drawing_port}/'],check=True)
        else:
            opened=unpack(portal.call(http.call_tool,'browser_open',{'summary':'Open an isolated Chrome fixture to verify background input without moving the user pointer.','workspace':workspace,'url':f'http://127.0.0.1:{drawing_port}/','width':1000,'height':750}))
            assert opened['isolated_profile'] and opened['delivery']=='chrome-devtools',opened
            chrome_pid=opened['pid']
        for _ in range(100):
            windows=computer(action='windows')['windows']
            target=next((w for w in windows if w['title']==f'Lessagent Background Drawing Test {drawing_port}'),None)
            if target and any(e['type']=='ready' for e in records): break
            time.sleep(.1)
        else: raise AssertionError('Chrome fixture did not become ready')
        chrome_pid=target['pid']
        # LaunchServices/Chrome may activate during first launch. Restore the
        # user's original process once, before measuring any control action.
        if native_probe:
            subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of first application process whose unix id is {before["frontmost_pid"]} to true'],check=True,capture_output=True)
            time.sleep(.6)
        else:
            # Managed browser creation is already background. Re-activating an
            # earlier app here can switch Spaces or override the user's work.
            assert opened['after']['frontmost_pid']!=chrome_pid,opened
            assert opened.get('automatic_screenshot'),opened
            assert opened['desktop']['frontmost_pid']!=chrome_pid,opened
        target_args=dict(window_id=target['window_id'],pid=chrome_pid,mode='background')
        shot_result=mcp(dict(target_args,action='screenshot',show_pointer=False,capture_path=OUTPUT_REL+'/before.png'))
        shot=unpack(shot_result)
        assert any(c.type=='image' and c.mimeType=='image/png' for c in shot_result.content)
        width,height=shot['screen_width'],shot['screen_height']
        lw,lh=shot['logical_width'],shot['logical_height']
        if not native_probe:
            assert shot['capture_backend']=='browser-surface-window-frame' and not shot['browser_chrome_captured'],shot
            assert width>=lw and height>=lh and abs(width/lw-height/lh)<.002,('Distorted or low-resolution observation',shot)
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
                report['in_flight_action']={'label':label,'arguments':args,'result':r}
                images=[c for c in raw.content if c.type=='image']
                assert images,('Missing automatic screenshot',label,r)
                image=images[0]
                assert image.mimeType=='image/png'
                assert base64.b64decode(image.data)==pathlib.Path(r['path']).read_bytes()
            assert time.monotonic()-started>=2,'Screenshot was returned before settling'
            assert r['automatic_screenshot'] and r['observation_delay_ms']==2000,r
            # macOS may return a differently scaled window image during a
            # desktop/display transition. Verify the actual PNG metadata, not
            # an earlier capture's scale. Input uses explicit screenshot sizes.
            observed_size=struct.unpack('>II',pathlib.Path(r['path']).read_bytes()[16:24])
            assert observed_size==(r['screen_width'],r['screen_height']),r
            assert r['logical_width']==lw and r['logical_height']==lh,('Window resized during test',r)
            assert min(observed_size)>0,r
            assert r['mode']=='background' and r['window_id']==target['window_id'] and r['pid']==chrome_pid,r
            # Document focus may be virtualized for the recipient's responder.
            # Actual OS foreground must never change to the background process,
            # including after the automatic-observation delay.
            for state_name in ['before','after','desktop']:
                assert r[state_name]['frontmost_pid']!=chrome_pid,(label,state_name,r)
            assert r.get('delivery')==('process-window' if native_probe else 'chrome-devtools'),r
            if not native_probe: assert r['native_input_events_posted']==0,r
            r['label']=label;r['transport']=transport;responses.append(r)
            events=since(start)
            assert all(e.get('trusted',True) for e in events),'Untrusted synthesized DOM events'
            if args['action'] in ('click','move','drag','scroll'):
                assert all(not any(e.get(k,False) for k in ('meta','ctrl','alt','shift')) for e in events),'Unexpected mouse modifiers'
            report.pop('in_flight_action',None)
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

        def plan(points, kind='click'):
            body=json.dumps({'kind':kind,'points':[[x,y-top] for x,y in points]}).encode()
            req=urllib.request.Request(f'http://127.0.0.1:{drawing_port}/plan',data=body,headers={'Content-Type':'application/json'})
            with urllib.request.urlopen(req,timeout=5) as response: plan_id=json.load(response)['plan']['id']
            deadline=time.monotonic()+8
            while not any(e['type']=='plan-ready' and e['planId']==plan_id for e in records):
                assert time.monotonic()<deadline,'Expected guides were not displayed'
                time.sleep(.05)

        # Corners, edges and center: numerical accuracy, not just a successful
        # tool response. Exercise all three entry points and screenshot scales.
        canvas_top=round(top+geometry['canvas']['y'])
        canvas_bottom=round(top+min(geometry['canvas']['y']+geometry['canvas']['height'],ready['innerHeight']))
        grid_y=[canvas_top+20,(canvas_top+canvas_bottom)//2,canvas_bottom-20]
        for index,(x,y) in enumerate((x,y) for y in grid_y for x in [35,360,690]):
            transport=['mcp-http','mcp-stdio','api'][index%3]
            space=['logical','Retina pixels','resized screenshot'][index//3]
            dimensions={} if index<3 else dict(screen_width=width if index<6 else width//2,screen_height=height if index<6 else height//2)
            px=x if not dimensions else round(x*dimensions['screen_width']/lw)
            py=y if not dimensions else round(y*dimensions['screen_height']/lh)
            plan([(x,y)])
            es=input_action(f'accuracy grid {index+1} / {space}',transport,action='click',x=px,y=py,**dimensions)
            for event_type in ['pointerdown','pointerup','click']:
                actual=[e for e in es if e['type']==event_type and e['target']=='canvas']
                assert len(actual)==1,(event_type,actual)
                error=math.hypot(actual[0]['x']-x,actual[0]['y']+top-y)
                accuracy.append({'kind':event_type,'transport':transport,'space':space,'error_css_px':error})
                assert error<=1.01,accuracy[-1]
        check('9-position grid / corners, edges, center / logical, Retina and resized screenshot accuracy <=1.01 CSS px')

        x1,y1=at('range',.25);x2,y2=at('range',.9)
        es=input_action('slider drag Retina pixels',action='drag',x=round(x1*width/lw),y=round(y1*height/lh),to_x=round(x2*width/lw),to_y=round(y2*height/lh),screen_width=width,screen_height=height,duration=.5)
        values=[int(e['value']) for e in es if e['type']=='input' and e['target']=='range']
        assert values and values[-1]>=85,es
        check('native slider drag / Retina coordinate conversion')

        def stroke(label,expected_path,**args):
            plan(expected_path,'drag')
            es=input_action(label,action='drag',**args)
            pointers=[e for e in es if e['type'].startswith('pointer') and e.get('target')=='canvas']
            down=[e for e in pointers if e['type']=='pointerdown'];up=[e for e in pointers if e['type']=='pointerup']
            assert len(down)==len(up)==1,(label,collections.Counter(e['type'] for e in pointers),pointers)
            assert down[0]['buttons']==1 and up[0]['buttons']==0,(label,pointers)
            if not native_probe:
                assert down[0]['pointerType']=='pen' and up[0]['pointerType']=='pen',(label,'not a virtual pen')
                assert down[0]['pointerId']==up[0]['pointerId'],(label,'pointer identity changed')
            active=[e for e in pointers if down[0]['seq']<e['seq']<up[0]['seq'] and e['type']=='pointermove']
            assert active and all(e['buttons']==1 for e in active),(label,active)
            if not native_probe:
                assert all(e['pointerId']==down[0]['pointerId'] and e['pointerType']=='pen' for e in active),(label,'mixed pointer streams')
            assert not any(e['type']=='pointercancel' for e in pointers),(label,pointers)
            for e in active:
                point=(e['x'],e['y']+top)
                error=min(line_distance(point,a,b) for a,b in zip(expected_path,expected_path[1:]))
                accuracy.append({'kind':'drag-path','label':label,'error_css_px':error})
                assert error<4,(label,'off-path movement',e)
            assert math.hypot(down[0]['x']-expected_path[0][0],down[0]['y']+top-expected_path[0][1])<3
            assert math.hypot(up[0]['x']-expected_path[-1][0],up[0]['y']+top-expected_path[-1][1])<3
            for vertex in expected_path:
                assert min(math.hypot(e['x']-vertex[0],e['y']+top-vertex[1]) for e in down+active+up)<12,(label,'missing path vertex',vertex)
            assert sum(e['type']=='stroke-complete' for e in es)==1,(label,es)
        stroke('horizontal shorthand',[(150,250),(700,250)],x=150,y=250,to_x=700,duration=.6)
        stroke('vertical shorthand',[(180,260),(180,500)],x=180,y=260,to_y=500,duration=.4)
        star=[[round(440+195*math.sin(i*4*math.pi/5)),round(435-195*math.cos(i*4*math.pi/5))] for i in range(6)]
        pixel_path=[[round(x*width/lw),round(y*height/lh)] for x,y in star]
        stroke('continuous star path',star,path=pixel_path,screen_width=width,screen_height=height,duration=3,capture_path=OUTPUT_REL+'/star.png')
        stroke('fast drag 50ms',[(200,650),(730,650)],start=[200,650],end=[730,650],duration_ms=50)
        for i in range(3):
            y=290+i*25;stroke('repeated drag '+str(i),[(210,y),(350,y)],x=210,y=y,to_x=350,to_y=y,duration=.1)
        check('horizontal / vertical / curved / fast / repeated continuous drags')
        star_result=next(r for r in responses if r['label']=='continuous star path')
        assert struct.unpack('>II',(OUTPUT/'star.png').read_bytes()[16:24])==(star_result['screen_width'],star_result['screen_height'])
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

        x,y=at('canvas',.3,.2)
        es=input_action('move',action='move',x=x,y=y)
        assert any(e['type']=='pointermove' and abs(e['x']-x)<2 and abs(e['y']-(y-top))<2 for e in es),es
        check('move / hover with no click')
        assert not any(e['type'] in ('pointerdown','pointerup','click') for e in es),es
        # All invalid requests must fail before any responder or pointer event.
        bad=[{'action':'click','x':5,'y':5},
             *[{'action':action,'mode':'desktop','x':10,'y':20,'to_x':30,'to_y':40,'distance':1,'key':'a','text':'test'} for action in ['move','click','drag','scroll','type','key','screenshot','windows']],
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
        # Independent adjacent scroll regions catch correct delta delivered at
        # the wrong cursor location. Only the addressed region may move.
        for index,element in enumerate(['scrollbox2','scrollbox']):
            x,y=at(element)
            plan([(x,y)],'scroll')
            es=input_action('isolated scroll '+element,['mcp-stdio','api'][index],action='scroll',x=round(x*width/lw),y=round(y*height/lh),screen_width=width,screen_height=height,distance=-4)
            details=[e for e in es if e['type']=='wheel-detail']
            assert details and all(e['target']==element for e in details),details
            assert any(e['type']=='scrolled' and e['target']==element for e in es),es
            assert not any(e['type']=='scrolled' and e['target']!=element for e in es),es
            for e in details:
                error=math.hypot(e['x']-x,e['y']+top-y)
                accuracy.append({'kind':'scroll','target':element,'error_css_px':error})
                assert error<=1.01,accuracy[-1]
        check('scroll location <=1.01 CSS px / adjacent panel isolation')

        # Observe the actual persistent panel, not just a cached screenshot.
        input_action('idle lifecycle start',action='move',x=100,y=600)
        pointer=responses[-1]['pointer']
        assert pointer['phase']=='active' and pointer['panel_visible'] and pointer['fill_alpha']==1,pointer
        epoch=time.monotonic()-pointer['idle_seconds']
        panel=pointer['panel_window_id']
        def capture_panel(name):
            path=OUTPUT/(name+'.png')
            subprocess.run(['/usr/sbin/screencapture','-x','-o','-l',str(panel),str(path)],check=True,capture_output=True)
            return path
        def blue_pixels(path):
            return sum(1 for r,g,b,a in Image.open(path).convert('RGBA').getdata() if a>100 and r<180 and g>130 and b>190 and b-r>40)
        active_path=capture_panel('pointer-active')
        assert blue_pixels(active_path)>30,'Live active pointer has no blue fill'
        for seconds,phase in [(9.0,'active'),(10.4,'transparent'),(29.0,'transparent'),(30.4,'hidden')]:
            time.sleep(max(0,epoch+seconds-time.monotonic()))
            pointer=unpack(mcp({'action':'windows'}))['pointer']
            idle_samples.append(pointer)
            assert pointer['phase']==phase,pointer
            assert pointer['panel_visible']==(phase!='hidden'),pointer
            assert pointer['fill_alpha']==(1 if phase=='active' else 0),pointer
            if seconds==10.4:
                transparent_path=capture_panel('pointer-transparent')
                assert blue_pixels(transparent_path)==0,'Idle pointer still has colored fill'
                assert any(a>0 for *_,a in Image.open(transparent_path).convert('RGBA').getdata()),'Pointer was removed instead of made transparent'
                shot=unpack(mcp(dict(target_args,action='screenshot',capture_path=OUTPUT_REL+'/idle-transparent.png')))
                assert shot['pointer_overlay'] and shot['pointer']['phase']=='transparent',shot
            print('PASS idle',seconds,phase,flush=True)
        shot=unpack(mcp(dict(target_args,action='screenshot',capture_path=OUTPUT_REL+'/idle-hidden.png')))
        assert not shot['pointer_overlay'] and shot['pointer']['phase']=='hidden',shot
        es=input_action('reactivate after idle',action='move',x=100,y=600)
        assert responses[-1]['pointer']['phase']=='active' and responses[-1]['pointer']['panel_visible']
        input_action('explicit hidden pointer',action='move',x=120,y=600,show_pointer=False)
        assert not responses[-1]['pointer']['panel_visible'] and not responses[-1]['pointer_overlay']
        input_action('restore visible pointer',action='move',x=140,y=600)
        assert responses[-1]['pointer']['phase']=='active' and responses[-1]['pointer_overlay']
        check('live overlay 10s transparent / 30s hidden / observations do not reset idle / reactivation / explicit hide')
        discovery=unpack(mcp({'action':'windows'}))
        helper_pid=discovery.get('helper_pid')
        if not native_probe:
            assert isinstance(helper_pid,int) and helper_pid>1,discovery
            parent=subprocess.check_output(['ps','-p',str(helper_pid),'-o','ppid='],text=True).strip()
            assert int(parent)==server.pid,('Refusing to stop a helper not owned by this test',helper_pid,parent)
            subprocess.run(['kill','-TERM',str(helper_pid)],check=True)
            time.sleep(.2)
            # Discovery is safe to retry after a dead helper; input is never replayed.
            restarted=mcp({'action':'windows'})
            if restarted.isError: restarted=mcp({'action':'windows'})
            restarted=unpack(restarted)
            assert restarted['helper_pid']!=helper_pid,restarted
            x,y=at('click-test')
            es=input_action('managed target survives helper restart',action='click',x=x,y=y)
            assert sum(e['type']=='button-click' for e in es)==1,es
            check('helper recovery / persisted browser target / no duplicate input replay')
            # Remove only this disposable test session's registration to model
            # an unmanaged Chrome window. The live page must receive no input.
            registration=root/'data/browsers'/f'window-{target["window_id"]}-{chrome_pid}.json'
            suspended=registration.with_suffix('.disabled')
            assert registration.is_file(),registration
            start=newest()
            registration.rename(suspended)
            try:
                for args in [dict(action='move',x=300,y=300),dict(action='click',x=300,y=300),
                             dict(action='drag',x=300,y=300,to_x=400,to_y=400,duration=.05),
                             dict(action='scroll',x=300,y=300,distance=-1),dict(action='key',key='a'),
                             dict(action='type',text='must not be entered')]:
                    rejected=mcp(target_args|args)
                    assert rejected.isError,rejected
                    assert any(c.type=='text' and 'Unmanaged Chrome is read-only' in c.text for c in rejected.content),rejected
                time.sleep(.2)
                assert not [e for e in since(start) if e['type'] in ['pointerdown','pointermove','pointerup','keydown','keyup','input','wheel']],since(start)
            finally:
                suspended.rename(registration)
            x,y=at('click-test')
            es=input_action('restored isolated channel',action='click',x=x,y=y)
            assert sum(e['type']=='button-click' for e in es)==1,es
            check('6 unmanaged Chrome input types rejected / no native fallback / restored registration works')

        stable=sum(r['before']['cursor_x']==r['after']['cursor_x'] and r['before']['cursor_y']==r['after']['cursor_y'] for r in responses)
        assert stable>0,'No stationary-cursor sample was available'
        report.update(passed=True,browser='Google Chrome',checks=checks,actions=len(responses),stationary_cursor_actions=stable,
                      foreground_target_seen=False,foreground_preserved_after_observation=True,
                      virtual_document_focus=any(e.get('focused') for e in records),all_input_events_trusted=all(e.get('trusted',True) for e in records),
                      accuracy_samples=len(accuracy),max_position_error_css_px=max((a['error_css_px'] for a in accuracy if a['kind']!='drag-path'),default=0),max_drag_cross_track_error_css_px=max((a['error_css_px'] for a in accuracy if a['kind']=='drag-path'),default=0),transports=sorted({r['transport'] for r in responses}),screenshots=['before.png','star.png'])
        print(json.dumps(report,indent=2),flush=True)
    except BaseException as error:
        report['failure']=str(error);report['traceback']=traceback.format_exc();raise
    finally:
        report['accuracy']=accuracy;report['idle_samples']=idle_samples;report['results']=responses;report['recorded_events']=sorted(records,key=lambda e:e.get('seq',0));report['checks']=checks
        (OUTPUT/'report.json').write_text(json.dumps(report,indent=2))
        clients.close()
        if chrome_pid:
            subprocess.run(['kill','-TERM',str(chrome_pid)],check=False,capture_output=True)
            for _ in range(100):
                result=subprocess.run(['kill','-0',str(chrome_pid)],capture_output=True)
                if result.returncode:break
                time.sleep(.05)
            else:subprocess.run(['kill','-KILL',str(chrome_pid)],capture_output=True)
            time.sleep(.2)
        # Do not reactivate an old app here: the user may have switched apps
        # while the test ran, and the background controls must respect that.
        server.send_signal(signal.SIGINT)
        try: server.wait(timeout=10)
        except subprocess.TimeoutExpired: server.kill();server.wait()
        drawing.shutdown();drawing.server_close();log.close()
        (OUTPUT/'server.log').write_text((root/'server.log').read_text())
