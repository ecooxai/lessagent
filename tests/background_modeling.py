#!/usr/bin/env python3
"""Real MCP modeling/drawing regression. Debug-only; no foreground activation.

Default: owned backend on a free non-production port. --live reuses the configured
backend with explicit user authorization; --keep leaves only the created windows.
All geometry changes and saving are performed through virtual input. The Blender
observer only reads scene data. Reports always retain failures and received events.
"""
import argparse
import base64
import json
import math
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.request

from control_lab import LabServer

ROOT = Path(__file__).resolve().parents[1]
TASK = 'Verify background Blender modeling and Chrome drawing'


def wait_for(fn, message, seconds=20):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            result = fn()
            if result:
                return result
        except (FileNotFoundError, json.JSONDecodeError, ConnectionError, OSError):
            pass
        time.sleep(.1)
    raise AssertionError(message)


class MCP:
    def __init__(self, port, data, report):
        self.base = f'http://127.0.0.1:{port}'
        self.data = data
        self.report_dir = report
        self.workspace = None
        self.calls = []
        self.build = None
        self.seq = 0

    def request(self, path, body=None):
        headers = {'Content-Type': 'application/json'}
        token = self.data / 'token'
        if token.exists():
            headers['Authorization'] = 'Bearer ' + token.read_text().strip()
        req = urllib.request.Request(self.base + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=90) as r:
            return json.load(r)

    def rpc(self, method, params):
        self.seq += 1
        envelope = self.request('/mcp', dict(jsonrpc='2.0', id=self.seq, method=method, params=params))
        assert 'error' not in envelope, envelope
        return envelope['result']

    def call(self, tool, *, expect_error=False, **args):
        record = dict(tool=tool, args=dict(args))
        if tool in {'app_open', 'browser_open', 'get_screenshot', 'virtual_pointer', 'virtual_keyboard'}:
            args['capture_path'] = str((self.report_dir/f'{len(self.calls):03d}-{tool}.png').relative_to(ROOT))
            if tool != 'app_open' and tool != 'browser_open':
                args['show_pointer'] = False
        args.update(summary='Verify the actual recipient result with exact-window virtual input.',
                    agent='background-modeling-regression', model='test-client', main_task=TASK,
                    current_task=tool, progress=70, quality=90,
                    current_timestamp=time.strftime('%Y-%m-%dT%H:%M:%S%z'))
        if self.workspace:
            args['workspace'] = self.workspace
        result = self.rpc('tools/call', dict(name=tool, arguments=args))
        value = result.get('structuredContent', {}).get('result', {})
        record.update(result={k:v for k,v in value.items() if k not in {'image','base64','data'}})
        self.calls.append(record)
        if expect_error:
            assert result.get('isError'), result
            return value
        assert not result.get('isError'), value
        assert not value.get('screenshot_error'), value
        if value.get('backend_build'):
            assert value['backend_build'] == self.build, 'GUI helper belongs to another build'
        images = [c for c in result.get('content', []) if c['type'] == 'image']
        if tool in {'app_open','browser_open','get_screenshot','virtual_pointer','virtual_keyboard'}:
            assert len(images) == 1, f'{tool}: no automatic image'
            raw = base64.b64decode(images[0]['data'])
            assert struct.unpack('>II', raw[16:24]) == (value['width'],value['height'])
            assert raw == Path(value['path']).read_bytes()
        return value

    def isolate(self, value, target):
        for name in ('before','after','desktop'):
            if name in value:
                assert value[name]['frontmost_pid'] != target['pid'], f'{name}: target became foreground'
        before, after = value.get('before'), value.get('after')
        if before and after:
            for key in ('physical_key_down_count','physical_left_down_count','physical_right_down_count'):
                if before.get(key) != after.get(key):
                    self.calls[-1]['concurrent_human_input'] = True
            motion = ('physical_mouse_move_count','physical_left_drag_count','physical_right_drag_count')
            if all(before.get(k) == after.get(k) for k in motion):
                assert (before['cursor_x'],before['cursor_y']) == (after['cursor_x'],after['cursor_y']), 'Physical cursor moved without physical motion'
        a,b=value.get('target_window_before'),value.get('target_window_after')
        if a and b:
            for key in ('window_id','pid','x','y','width','height'):
                assert a.get(key) == b.get(key), f'Target presentation changed: {key}'


def exercise(args):
    assert sys.platform == 'darwin', 'macOS GUI session is required'
    assert 'debug' in args.binary.resolve().parts, 'Use a debug build only'
    report_dir = (ROOT/args.report_dir).resolve()
    report_dir.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, checks=[], report_dir=str(report_dir))
    owned=[]; backend=None; temporary=None; lab=None; log=None; mcp=None
    try:
        if args.live:
            port=args.port; data=args.data_dir.expanduser().resolve()
        else:
            temporary=tempfile.TemporaryDirectory(prefix='lessagent-modeling-')
            data=Path(temporary.name)/'data'
            with socket.socket() as sock:
                sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
            assert port != 3210
            log=(report_dir/'server.log').open('w')
            backend=subprocess.Popen([str(args.binary.resolve()),'serve','--port',str(port),'--data-dir',str(data)], stdout=log,stderr=log)
        mcp=MCP(port,data,report_dir)
        health=wait_for(lambda:mcp.request('/health'),'Backend did not become ready')
        mcp.build=health.get('build')
        expected=json.loads(subprocess.check_output([str(args.binary.resolve()),'build-info'],text=True))
        assert mcp.build == expected, ('Running backend is stale',mcp.build,expected)
        report.update(port=port, backend_pid=health['pid'], build=mcp.build)
        if not args.live:
            settings=mcp.request('/api/state')['settings']; settings['computer_enabled']=True
            mcp.request('/api/action/settings',settings)
        mcp.rpc('initialize',dict(protocolVersion='2025-11-25',capabilities={},clientInfo=dict(name='modeling-test',version='1')))
        tools={t['name']:t for t in mcp.rpc('tools/list',{})['tools']}
        for name in ('browser_open','app_open','virtual_pointer','virtual_keyboard'):
            assert name in tools
            for field in ('summary','current_task','main_task','progress','quality'):
                assert field in tools[name]['inputSchema']['required']
        assert 'middle' in tools['virtual_pointer']['inputSchema']['properties']['button']['enum']
        assert 'modifiers' in tools['virtual_pointer']['inputSchema']['properties']
        mcp.workspace=mcp.call('workspace_open',path=str(ROOT))['id']
        before=mcp.call('list_windows'); assert before['accessibility'] and before['screen_recording']
        prior={w['pid'] for w in before['windows']}
        if not args.chrome_only:
            shot=mcp.call('app_open',app='Blender',new_instance=True)
            target=dict(window_id=shot['window_id'],pid=shot['pid'],mode='background')
            assert target['pid'] not in prior, 'Test must not edit an existing Blender'
            owned.append(target['pid']); mcp.isolate(shot,target)
            def pointer(action='move', uv=(.4,.5), to=None, **kw):
                nonlocal shot
                args2=dict(target,action=action,x=round(shot['width']*uv[0]),y=round(shot['height']*uv[1]),screen_width=shot['width'],screen_height=shot['height'],**kw)
                if to:
                    args2.update(to_x=round(shot['width']*to[0]),to_y=round(shot['height']*to[1]))
                shot=mcp.call('virtual_pointer',**args2); mcp.isolate(shot,target)
                return shot
            def key(name):
                nonlocal shot
                shot=mcp.call('virtual_keyboard',**target,action='key',key=name);mcp.isolate(shot,target)
                assert shot['context_events_posted']==1, 'Blender key lacked virtual editor context'
                return shot
            def text(value):
                nonlocal shot
                shot=mcp.call('virtual_keyboard',**target,action='type',text=value);mcp.isolate(shot,target)
                assert shot['context_events_posted']==1
                return shot
            pointer('click',(.13,.54))
            # Startup can replace the native window. Re-select only this owned PID.
            ws=[w for w in mcp.call('list_windows')['windows'] if w['pid']==target['pid'] and w.get('title')]
            assert len(ws)==1, ws
            target['window_id']=ws[0]['window_id'];shot=mcp.call('get_screenshot',**target)
            pointer();key('shift+f4')
            statefile=report_dir/'blender-state.json'
            statefile.unlink(missing_ok=True)
            observer=ROOT/'tests/fixtures/blender_model_probe.py'
            command=f"exec(compile(open({str(observer)!r}).read(),'probe','exec'),{{'STATE_PATH':{str(statefile)!r}}})"
            text(command);key('enter')
            def state(): return json.loads(statefile.read_text())
            wait_for(lambda:statefile.exists(),'Read-only Blender observer did not initialize')
            key('shift+f5')
            def active():
                s=state();return next(o for o in s['objects'] if o['name']==s['active'])
            def search(operator):
                key('f3');text(operator);key('enter')
            search('Add Cube')
            created=active(); assert created['type']=='MESH' and created['vertices']==8
            # Blender may retain a same-named object from its user startup
            # file. Use a per-process name so the regression verifies rename
            # delivery rather than Blender's automatic .001 disambiguation.
            model_name=f'BG_Model_Base_{target["pid"]}'
            key('f2');text(model_name);key('enter')
            assert active()['name']==model_name
            start=list(active()['location'])
            key('g');key('x');text('-2.5');key('enter')
            assert abs(active()['location'][0]-start[0]+2.5)<1e-4, active()
            report['checks'].append('Negative decimal translation committed through real background input')
            # No intervening test pointer move: this is the regression that broke.
            base=active()
            key('shift+d');key('z');text('2');key('enter')
            duplicate=active()
            assert duplicate['name']!=base['name'] and abs(duplicate['location'][2]-base['location'][2]-2)<1e-4, duplicate
            key('s');text('0.5');key('enter')
            assert all(abs(v-.5)<1e-4 for v in active()['scale']),active()
            key('r');key('z');text('45');key('enter')
            assert abs(active()['rotation'][2]-math.pi/4)<1e-4,active()
            key('tab');key('a');search('Subdivide');key('tab')
            assert active()['vertices']>8 and active()['faces']>6,active()
            report['checks'].append('Post-transform duplicate, scale, rotation and edit-mode subdivision verified in mesh data')
            key('numpad1')
            def view(): return next(a['view'] for a in state()['windows'][0]['areas'] if a['type']=='VIEW_3D')
            beforeview=view();pointer('drag',(.4,.5),(.48,.58),button='middle',duration=.4)
            assert view()['rotation']!=beforeview['rotation'],'Middle drag did not orbit'
            beforeview=view();pointer('drag',(.4,.5),(.45,.52),button='middle',modifiers=['shift'],duration=.4)
            assert view()['location']!=beforeview['location'],'Shift+middle drag did not pan'
            assert any(e['type']=='MIDDLEMOUSE' for e in state()['events']), 'No middle-button event received'
            report['checks'].append('Numeric keypad view, middle-button orbit and Shift+middle pan accepted while backgrounded')
            # Put the pointer away from the canvas center before framing the object.
            pointer(); key('numpaddecimal')
            blend=report_dir/'background-model.blend'
            key('shift+f4');text(f"bpy.ops.wm.save_as_mainfile(filepath={str(blend)!r},check_existing=False)");key('enter')
            wait_for(lambda:blend.is_file() and blend.stat().st_size>1000,'Scene was not saved')
            key('shift+f5');shot=mcp.call('get_screenshot',**target)
            (report_dir/'blender-final.png').write_bytes(Path(shot['path']).read_bytes())
            report['blender']=dict(target=target,objects=state()['objects'],blend=str(blend),screenshot=str(report_dir/'blender-final.png'),capture_quality=shot.get('capture_quality'))
            report['checks'].append('Saved actual modeled geometry to .blend using the background console')
            print('Blender modeling PASS',flush=True)
        lab=LabServer();threading.Thread(target=lab.serve_forever,daemon=True).start()
        chrome=mcp.call('browser_open',url=f'http://127.0.0.1:{lab.server_port}/?purpose=test-client+background-circle',new_profile=True,width=1000,height=600)
        target=dict(window_id=chrome['window_id'],pid=chrome['pid'],mode='background');owned.append(target['pid']);mcp.isolate(chrome,target)
        def events():
            with lab.lock: return list(lab.events)
        ready=wait_for(lambda:next((e for e in reversed(events()) if e.get('type')=='ready'),None),'Chrome page did not become ready')
        canvas=ready['geometry']['canvas'];cx=canvas['x']+canvas['width']*.42;cy=canvas['y']+min(canvas['height']*.35,110);radius=65
        top=chrome['logical_height']-ready['innerHeight']
        path=[[round((cx+radius*math.cos(i*2*math.pi/64))*chrome['width']/chrome['logical_width']),round((top+cy+radius*math.sin(i*2*math.pi/64))*chrome['height']/chrome['logical_height'])] for i in range(65)]
        seq=max((e.get('seq',0) for e in events()),default=0)
        drawn=mcp.call('virtual_pointer',**target,action='drag',path=path,duration=1.2,screen_width=chrome['width'],screen_height=chrome['height'],show_pointer=True)
        mcp.isolate(drawn,target); assert drawn['delivery']=='chrome-devtools' and drawn['native_input_events_posted']==0
        wait_for(lambda:any(e.get('type')=='stroke-complete' and e.get('seq',0)>seq for e in events()),'Chrome did not complete a stroke')
        received=[e for e in events() if e.get('seq',0)>seq]
        for kind in ('pointerdown','pointerup','stroke-complete'):
            assert sum(e['type']==kind for e in received)==1,(kind,received)
        motion=[e for e in received if e['type']=='pointermove' and e.get('buttons')==1]
        assert len(motion)>=20
        assert all(e['trusted'] for e in motion)
        error=max(abs(math.hypot(e['x']-cx,e['y']-cy)-radius) for e in motion)
        assert error<2, error
        (report_dir/'chrome-circle.png').write_bytes(Path(drawn['path']).read_bytes())
        (report_dir/'chrome-events.json').write_text(json.dumps(received,indent=2))
        report['chrome']=dict(target=target,radial_error_css_px=error,held_moves=len(motion),screenshot=str(report_dir/'chrome-circle.png'))
        report['checks'].append('Chrome drew one trusted continuous circle with held-button state and <2px radial error')
        report['passed']=True
        print('Chrome drawing PASS',flush=True)
    except Exception as error:
        report['error']=str(error);report['traceback']=traceback.format_exc();raise
    finally:
        if mcp: report['calls']=mcp.calls
        (report_dir/'report.json').write_text(json.dumps(report,indent=2,ensure_ascii=False))
        if not args.keep or not report['passed']:
            for pid in reversed(owned):
                try: os.kill(pid,signal.SIGTERM)
                except ProcessLookupError: pass
            time.sleep(.5)
        if lab:lab.shutdown();lab.server_close()
        if backend:
            backend.terminate()
            try:backend.wait(timeout=10)
            except subprocess.TimeoutExpired:backend.kill();backend.wait()
        if log:log.close()
        if temporary:
            # Chrome helpers can take a moment to flush their disposable profile.
            try:temporary.cleanup()
            except OSError: pass
    return report

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary',type=Path,nargs='?',default=Path('target/debug/lessagent'))
    parser.add_argument('--report-dir',type=Path,default=Path('output/background-modeling'))
    parser.add_argument('--live',action='store_true')
    parser.add_argument('--keep',action='store_true')
    parser.add_argument('--chrome-only',action='store_true')
    parser.add_argument('--port',type=int,default=3210)
    parser.add_argument('--data-dir',type=Path,default=Path.home()/'.local/share/lessagent')
    result=exercise(parser.parse_args())
    print(json.dumps({k:v for k,v in result.items() if k!='calls'},indent=2))
