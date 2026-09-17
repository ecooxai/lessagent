#!/usr/bin/env python3
"""Automatic macOS regression for restored managed browser/input tools.

Called by cargo test. Uses the production native launcher with a disposable
managed profile, never personal Chrome data. Full MCP transport/image coverage
lives in computer_background.py and browser_profile_virtual_tools.py.
"""
import collections
import json
import math
import os
import pathlib
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

from control_lab import LabServer

ROOT = pathlib.Path(__file__).resolve().parents[1]
HELPER = pathlib.Path(sys.argv[1]).resolve()
REPORT_DIR = ROOT / os.environ.get('LESSAGENT_CHROME_REPORT_DIR', 'output/chrome-background-pointer')


def wait_for(fn, message, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = fn()
        if result:
            return result
        time.sleep(.05)
    raise AssertionError(message)


def main():
    assert sys.platform == 'darwin' and HELPER.is_file()
    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'helper': str(HELPER), 'checks': [], 'actions': []}
    configured_root = os.environ.get('LESSAGENT_TEST_BROWSER_ROOT')
    temporary = pathlib.Path(tempfile.mkdtemp(prefix='lessagent-restored-gui-'))
    browser_root = pathlib.Path(configured_root).expanduser() if configured_root else temporary / 'browsers'
    lab = LabServer()
    threading.Thread(target=lab.serve_forever, daemon=True).start()
    log = (REPORT_DIR / 'helper.log').open('w')
    helper = subprocess.Popen([str(HELPER), '--server'], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1)
    owned_pids = set()

    def raw_call(**args):
        args['_browser_root'] = str(browser_root)
        helper.stdin.write(json.dumps(args) + '\n')
        helper.stdin.flush()
        if not select.select([helper.stdout], [], [], 45)[0]:
            raise TimeoutError('Native helper did not reply within 45 seconds')
        line = helper.stdout.readline()
        assert line, f'Native helper exited: {helper.poll()}'
        return json.loads(line)

    def call(**args):
        value = raw_call(**args)
        assert 'error' not in value, value
        return value

    def snapshot():
        with lab.lock:
            return sorted(list(lab.events), key=lambda e: e.get('seq', 0))

    def latest():
        return max((e.get('seq', 0) for e in snapshot()), default=0)

    def since(seq):
        return [e for e in snapshot() if e.get('seq', 0) > seq]

    def assert_isolation(result, target):
        assert result['delivery'] == 'chrome-devtools', result
        assert result['native_input_events_posted'] == 0, result
        assert not result['responder_lease'], result
        # Chrome may legitimately promote a visible window while processing a
        # drag. Background delivery is established by the DevTools route and
        # the absence of native/global input, not by forcing focus to remain
        # on another application.
        # Human activity is allowed. When no physical motion happened, require
        # the physical cursor to stay fixed; browser input never posts HID events.
        before, after = result['before'], result['after']
        motion = ['physical_mouse_move_count', 'physical_left_drag_count', 'physical_right_drag_count']
        if all(before.get(k) == after.get(k) for k in motion):
            assert (before['cursor_x'], before['cursor_y']) == (after['cursor_x'], after['cursor_y'])

    def act(label, target, terminal_event, **args):
        seq = latest()
        result = call(**target, **args)
        assert_isolation(result, target)
        wait_for(lambda: any(e.get('type') == terminal_event for e in since(seq)),
                 f'{label}: page did not receive {terminal_event}', 5)
        time.sleep(.1)
        events = since(seq)
        inputs = [e for e in events if e.get('type') in {'pointermove', 'pointerdown', 'pointerup', 'click', 'input', 'keydown', 'keyup'}]
        assert inputs and all(e.get('trusted') is True for e in inputs), (label, events)
        report['actions'].append({'label': label, 'events': dict(collections.Counter(e['type'] for e in events))})
        return events

    try:
        initial = call(action='windows')
        assert initial['accessibility'] and initial['screen_recording'], 'Grant Accessibility and Screen Recording before local GUI tests'
        opened = call(action='browser_open', url=f'http://127.0.0.1:{lab.server_port}/?purpose=test-client+restored-gui', width=1000, height=600)
        owned_pids.add(opened['pid'])
        assert opened['control_available'] and opened['isolated_profile'] and opened['persistent_profile'], opened
        assert opened['profile_source'] == 'lessagent-managed', opened
        assert opened['after']['frontmost_pid'] != opened['pid'], opened
        target = dict(window_id=opened['window_id'], pid=opened['pid'], mode='background', show_pointer=False)
        ready = wait_for(lambda: next((e for e in reversed(snapshot()) if e['type'] == 'ready'), None), 'Page not ready')
        shot = call(**target, action='screenshot', capture_path=str(REPORT_DIR/'before.png'))
        sx, sy = shot['screen_width']/shot['logical_width'], shot['screen_height']/shot['logical_height']
        toolbar = shot['logical_height'] - ready['innerHeight']

        def point(x, y):
            return [round(x*sx), round((toolbar+y)*sy)]

        def element(name):
            r = ready['geometry'][name]
            return point(r['x']+r['width']/2, r['y']+r['height']/2)

        dims = dict(screen_width=shot['screen_width'], screen_height=shot['screen_height'])
        r = ready['geometry']['canvas']
        visible_height = min(r['height'], ready['innerHeight']-r['y'])
        center = [round(r['x']+r['width']*.45), round(r['y']+visible_height*.5)]
        radius = min(70, visible_height/3)
        assert radius > 15
        x, y = point(*center)
        events = act('move without click', target, 'pointermove', action='move', x=x, y=y, **dims)
        assert not any(e['type'] in ['pointerdown', 'pointerup', 'click'] for e in events)
        for run in range(2):
            events = act(f'repeated click {run+1}', target, 'click', action='click', x=x, y=y, **dims)
            for name in ['pointerdown', 'pointerup', 'click']:
                chosen = [e for e in events if e['type'] == name and e.get('target') == 'canvas']
                assert len(chosen) == 1, (name, events)
                e = chosen[0]
                assert abs(e['x']-center[0]) <= 1 and abs(e['y']-center[1]) <= 1, e
                assert e['buttons'] == (1 if name == 'pointerdown' else 0), e
        report['checks'].append('Repeated clicks: one trusted down/up/click, expected coordinates and button masks')

        path_css = [[center[0]+radius*math.cos(i*2*math.pi/48), center[1]+radius*math.sin(i*2*math.pi/48)] for i in range(49)]
        path = [point(*p) for p in path_css]
        signatures = []
        for run in range(2):
            events = act(f'repeated circle {run+1}', target, 'stroke-complete', action='drag',
                         x=path[0][0], y=path[0][1], path=path, duration=.65, **dims)
            assert sum(e['type']=='pointerdown' for e in events) == 1
            assert sum(e['type']=='pointerup' for e in events) == 1
            assert sum(e['type']=='stroke-complete' for e in events) == 1
            moves = [e for e in events if e['type']=='pointermove' and e.get('buttons')==1]
            assert len(moves) >= 10, events
            error = max(abs(math.hypot(e['x']-center[0], e['y']-center[1])-radius) for e in moves)
            assert error <= 2, error
            signatures.append({'downs': 1, 'ups': 1, 'strokes': 1, 'radius_css_px': radius})
        assert signatures[0] == signatures[1]
        report['checks'].append('Repeated fixed circle: continuous held-button path, one stroke, radial error <=2 CSS pixels')
        report['repeat_signatures'] = signatures

        x, y = element('name')
        act('focus input', target, 'click', action='click', x=x, y=y, **dims)
        act('type Unicode', target, 'input', action='type', text='Rollback ✓ 猫🐈')
        act('select all', target, 'keydown', action='key', key='cmd+a')
        events = act('replace selection', target, 'input', action='type', text='restored')
        assert [e.get('value') for e in events if e['type']=='input'][-1] == 'restored', events
        report['checks'].append('Managed keyboard: Unicode including emoji and Command+A verified in the page')
        call(**target, action='screenshot', capture_path=str(REPORT_DIR/'after.png'))

        # Missing registration must not silently enter any native Chrome path.
        registration = browser_root/f"window-{target['window_id']}-{target['pid']}.json"
        record = registration.read_bytes(); registration.unlink()
        start = latest()
        try:
            for args in [dict(action='move', x=x, y=y), dict(action='click', x=x, y=y),
                         dict(action='drag', x=x, y=y, to_x=x+10, to_y=y+10),
                         dict(action='scroll', x=x, y=y, delta=1), dict(action='key', key='a'), dict(action='type', text='blocked')]:
                rejected = raw_call(**target, **args)
                assert 'Unmanaged Chrome is read-only' in rejected.get('error', ''), rejected
            time.sleep(.2)
            assert not [e for e in since(start) if e['type'] in {'pointermove','pointerdown','pointerup','click','wheel','keydown','keyup','input'}]
        finally:
            registration.write_bytes(record)
        report['checks'].append('Six unregistered Chrome actions rejected before input; no native/global fallback')
        report.update(passed=True, remote_debugging_delivery='chrome-devtools', no_debug_delivery='rejected-before-input')
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        # Stop only test-owned browser roots, including partial launch failures.
        keep_browser = os.environ.get('LESSAGENT_KEEP_CHROME') == '1'
        for row in subprocess.check_output(['ps','-ax','-o','pid=','-o','command='], text=True).splitlines():
            pid, _, cmd = row.strip().partition(' ')
            if cmd.startswith('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome ') and f'--user-data-dir={browser_root}/' in cmd:
                owned_pids.add(int(pid))
        if not keep_browser:
            for pid in owned_pids:
                try: os.kill(pid, signal.SIGTERM)
                except ProcessLookupError: pass
        alive=[]
        if not keep_browser:
            for _ in range(80):
                alive=[]
                for pid in owned_pids:
                    try: os.kill(pid,0); alive.append(pid)
                    except ProcessLookupError: pass
                if not alive: break
                time.sleep(.05)
            for pid in alive if owned_pids else []:
                try: os.kill(pid,signal.SIGKILL)
                except ProcessLookupError: pass
        try: helper.stdin.close(); helper.wait(timeout=5)
        except Exception: helper.kill(); helper.wait(timeout=5)
        lab.shutdown(); lab.server_close(); log.close()
        (REPORT_DIR/'report.json').write_text(json.dumps(report, indent=2))
        (REPORT_DIR/'events.json').write_text(json.dumps(snapshot(), indent=2))
        if not keep_browser:
            shutil.rmtree(temporary, ignore_errors=True)
        else:
            report['kept_browser_root'] = str(browser_root)
    print(json.dumps(report, indent=2), flush=True)


if __name__ == '__main__':
    main()
