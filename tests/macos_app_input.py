#!/usr/bin/env python3
"""Real MCP Chrome/Blender input matrix; disposable debug server and app instances.

uv run --with 'mcp>=1.20,<2' tests/macos_app_input.py target/debug/lessagent
Use --apps blender or --apps chrome to isolate a failure. Test setup explicitly
foregrounds only disposable target instances; production input never does so.
"""
import argparse
import base64
from contextlib import ExitStack
import datetime
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import signal
import socket
import struct
import subprocess
import tempfile
import threading
import time
import traceback
import urllib.request

from anyio.from_thread import start_blocking_portal
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client
from control_lab import LabServer

ROOT = Path(__file__).resolve().parents[1]


def wait_for(fn, message, seconds=20):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            value = fn()
            if value:
                return value
        except (OSError, ValueError, KeyError) as error:
            last = repr(error)
        time.sleep(.1)
    raise AssertionError(f'{message}: {last}')


def run(binary, apps, report_dir, chrome_devtools=False):
    report_dir.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, binary=str(binary), binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), checks=[], actions=[], chrome_devtools=chrome_devtools)
    with tempfile.TemporaryDirectory(prefix='lessagent-input-matrix-') as tmp:
        temp = Path(tmp)
        data = temp / 'data'
        pids = set()
        labs = []
        original_front = None
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        log = (report_dir / 'server.log').open('w')
        backend = subprocess.Popen([str(binary), 'serve', '--port', str(port),
                                    '--data-dir', str(data)], stdout=log, stderr=log)
        report['isolated_port'] = port
        clients = ExitStack()
        try:
            def request(path, body=None):
                req = urllib.request.Request(f'http://127.0.0.1:{port}{path}',
                    data=None if body is None else json.dumps(body).encode(),
                    headers={'Content-Type': 'application/json'})
                with urllib.request.urlopen(req, timeout=40) as response:
                    return json.load(response)

            state = wait_for(lambda: request('/api/state'), 'debug backend readiness')
            state['settings']['computer_enabled'] = True
            request('/api/action/settings', state['settings'])
            workspace = request('/api/action/workspace_open', {'path': str(ROOT)})['id']
            portal = clients.enter_context(start_blocking_portal())
            streams = clients.enter_context(portal.wrap_async_context_manager(streamablehttp_client(
                f'http://127.0.0.1:{port}/mcp',
                headers={'Authorization': 'Bearer ' + (data / 'token').read_text().strip()})))
            client = clients.enter_context(portal.wrap_async_context_manager(ClientSession(streams[0], streams[1])))
            portal.call(client.initialize)
            index = 0

            def call(tool, *, expect_error=False, setup_mode=None, **args):
                nonlocal index
                if setup_mode is not None:
                    ensure_mode(setup_mode, args)
                index += 1
                if tool in {'virtual_pointer', 'virtual_keyboard', 'get_screenshot'}:
                    args.setdefault('show_pointer', False)
                    args['capture_path'] = str((report_dir / f'{index:03d}-{tool}.png').relative_to(ROOT))
                result = portal.call(client.call_tool, tool, dict(workspace=workspace,
                    summary='Verify real recipient state and input isolation in a disposable app.',
                    agent='macos-input-matrix', model='test-client',
                    main_task='Verify Chrome and Blender foreground/background input',
                    current_task=tool, progress=50, quality=0,
                    current_timestamp=datetime.datetime.now().astimezone().isoformat(), **args))
                if expect_error:
                    assert result.isError, ('expected rejection', tool, args)
                    error = result.structuredContent['result']['error']
                    report.setdefault('rejections', []).append(dict(tool=tool, error=error, args=args))
                    return error
                if result.isError:
                    report['failed_call'] = dict(tool=tool, args=args, error=str(result))
                    if tool != 'list_windows':
                        report['failure_windows'] = call('list_windows')
                assert not result.isError, (tool, result)
                value = result.structuredContent['result']
                if tool == 'get_screenshot':
                    report.setdefault('observations', []).append({k:v for k,v in value.items() if k not in {'image', 'data', 'base64'}})
                if tool in {'virtual_pointer', 'virtual_keyboard', 'get_screenshot'}:
                    images = [b for b in result.content if b.type == 'image']
                    assert len(images) == 1, ('missing screenshot', tool)
                    raw = base64.b64decode(images[0].data)
                    assert raw[:8] == b'\x89PNG\r\n\x1a\n'
                    assert struct.unpack('>II', raw[16:24]) == (value['width'], value['height'])
                    assert raw == Path(value['path']).read_bytes()
                return value

            initial = call('list_windows')
            assert initial['accessibility'] and initial['screen_recording']
            original_front = initial['desktop']['frontmost_pid']

            def activate(pid):
                # Deliberate foreground-test setup, never a production fallback.
                subprocess.run(['osascript', '-e', f'tell application "System Events" to set frontmost of first application process whose unix id is {pid} to true'],
                               check=True, capture_output=True, timeout=10)
                wait_for(lambda: call('list_windows')['desktop']['frontmost_pid'] == pid,
                         'foreground setup did not settle')
                time.sleep(.3)

            def ensure_mode(mode, target):
                # Explicit test setup only. A human may switch apps between
                # assertions; establish the requested test condition again.
                desktop = call('list_windows')['desktop']
                focused = desktop.get('focused_window_id') or desktop['front_window_id']
                target_foreground = desktop['frontmost_pid'] == target['pid'] and focused == target['window_id']
                if mode == 'foreground' and not target_foreground:
                    activate(target['pid'])
                elif mode == 'background' and target_foreground:
                    activate(original_front)

            def record(label, value, mode, target):
                assert value['before']['frontmost_pid'] == value['after']['frontmost_pid'], (label, value)
                focused = value['before'].get('focused_window_id') or value['before']['front_window_id']
                is_foreground = value['before']['frontmost_pid'] == target['pid'] and focused == target['window_id']
                assert is_foreground == (mode == 'foreground'), (label, value)
                a, b = value['before'], value['after']
                counters = ('physical_left_down_count', 'physical_right_down_count', 'physical_key_down_count')
                counters += ('physical_mouse_move_count', 'physical_left_drag_count', 'physical_right_drag_count')
                human_input = any(a.get(k) != b.get(k) for k in counters)
                if not human_input and mode == 'background':
                    assert a['front_window_id'] == b['front_window_id'], (label, a, b)
                    if a.get('focused_window_id') and b.get('focused_window_id'):
                        assert a['focused_window_id'] == b['focused_window_id'], (label, a, b)
                if not human_input:
                    assert (a['cursor_x'], a['cursor_y']) == (b['cursor_x'], b['cursor_y']), (label, a, b)
                # Counters distinguish actual concurrent human movement from
                # cursor warping; do not count interrupted isolation as proven.
                report['actions'].append(dict(label=label, mode=mode, delivery=value.get('delivery'),
                    before=a, after=b, concurrent_physical_input=human_input, screenshot=value['path'],
                    events_posted=value.get('input_events_posted'), target_window_before=value.get('target_window_before'),
                    target_window_after=value.get('target_window_after')))
                print('INPUT', label, flush=True)

            if 'chrome' in apps:
                lab = LabServer(); labs.append(lab)
                threading.Thread(target=lab.serve_forever, daemon=True).start()
                profile = temp / 'chrome-profile'
                url = f'http://127.0.0.1:{lab.server_port}/?purpose=test-client+foreground-background-input'
                launch = ['/usr/bin/open', '-g', '-n', '-a', 'Google Chrome', '--args',
                          f'--user-data-dir={profile}', '--no-first-run', '--no-default-browser-check',
                          '--disable-background-networking', '--new-window', '--window-size=1000,600', url]
                if chrome_devtools:
                    launch[launch.index('--new-window'):launch.index('--new-window')] = ['--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0']
                call('bash', command=shlex.join(launch), wait_ms=10000)
                ready = wait_for(lambda: next((e for e in reversed(lab.events) if e.get('type') == 'ready'), None), 'Chrome page readiness')
                def chrome_window():
                    matches = [w for w in call('list_windows')['windows'] if w.get('title') == f'Lessagent Background Drawing Test {lab.server_port}']
                    return matches[0] if len(matches) == 1 else None
                win = wait_for(chrome_window, 'exact Chrome window')
                pids.add(win['pid']); target = dict(window_id=win['window_id'], pid=win['pid'], mode='background')
                assert call('list_windows')['desktop']['frontmost_pid'] == original_front, 'Chrome launch stole focus'
                assert (profile / 'DevToolsActivePort').exists() == chrome_devtools
                report['checks'].append('MCP Bash opened Chrome in background; DevTools=' + str(chrome_devtools))
                shot = call('get_screenshot', **target)
                report['chrome_ready'] = ready
                report['chrome_window'] = win

                def chrome_action(label, mode, action, *, control=None, xy=None, to=None, **kwargs):
                    nonlocal shot
                    seq = max((e.get('seq', 0) for e in lab.events), default=0)
                    tool = 'virtual_keyboard' if action in {'key', 'type'} else 'virtual_pointer'
                    args = dict(target, action=action, **kwargs)
                    if tool == 'virtual_pointer':
                        bar = ready['outerHeight'] - ready['innerHeight']
                        if control:
                            r = ready['geometry'][control]; xy = (r['x'] + r['width']*.5, r['y'] + r['height']*.5)
                        def pixel(point):
                            return (round(point[0] * shot['width'] / ready['outerWidth']),
                                    round((point[1] + bar) * shot['height'] / ready['outerHeight']))
                        args['x'], args['y'] = pixel(xy)
                        if to: args['to_x'], args['to_y'] = pixel(to)
                        args.update(screen_width=shot['width'], screen_height=shot['height'])
                    value = call(tool, setup_mode=mode, **args); shot = value
                    record('Chrome ' + mode + ': ' + label, value, mode, target)
                    time.sleep(.15)
                    events = sorted((e for e in lab.events if e.get('seq', 0) > seq), key=lambda e:e['seq'])
                    assert all(e.get('trusted', True) for e in events), events
                    report['actions'][-1]['recipient_events'] = events
                    return events

                for mode in ('background', 'foreground', 'background'):
                    activate(win['pid'] if mode == 'foreground' else original_front)
                    events = chrome_action('canvas click', mode, 'click', xy=(400, 300))
                    for kind in ('pointerdown', 'pointerup', 'click'):
                        assert sum(e.get('type') == kind for e in events) == 1, (kind, events)
                    if chrome_devtools:
                        events = chrome_action('continuous drag', mode, 'drag', xy=(350, 280), to=(520, 380), duration=.45)
                        down = next(i for i,e in enumerate(events) if e.get('type') == 'pointerdown')
                        up = next(i for i,e in enumerate(events) if e.get('type') == 'pointerup')
                        held = [e for e in events[down+1:up] if e.get('type') == 'pointermove']
                        assert held and all(e['buttons'] == 1 for e in held), events
                        assert sum(e.get('type') == 'pointerdown' for e in events) == 1
                        assert sum(e.get('type') == 'pointerup' for e in events) == 1
                    else:
                        seq = max((e.get('seq', 0) for e in lab.events), default=0)
                        error = call('virtual_pointer', expect_error=True, setup_mode=mode, **target, action='drag',
                             x=200, y=200, to_x=300, to_y=300)
                        assert 'held-button state' in error, error
                        time.sleep(.1)
                        assert not any(e.get('seq', 0) > seq and e.get('type') in ('pointerdown','pointerup','pointermove') for e in lab.events)
                        report['checks'].append('Chrome native drag rejected without emitting input: ' + mode)
                    events = chrome_action('right click', mode, 'click', xy=(410, 310), button='right')
                    assert sum(e.get('type') == 'contextmenu' for e in events) == 1, events
                    focus_events = chrome_action('text field focus', mode, 'click', control='name')
                    prefix = suffix = ''
                    if chrome_devtools or mode == 'foreground':
                        chrome_action('select all shortcut', mode, 'key', key='cmd+a')
                    else:
                        focused_event = next(e for e in reversed(focus_events) if e.get('type') == 'click' and e.get('target') == 'name')
                        current = focused_event['value'].encode('utf-16-le')
                        prefix = current[:2*focused_event['selectionStart']].decode('utf-16-le')
                        suffix = current[2*focused_event['selectionEnd']:].decode('utf-16-le')
                        seq = max((e.get('seq', 0) for e in lab.events), default=0)
                        error = call('virtual_keyboard', expect_error=True, setup_mode=mode, **target, action='key', key='cmd+a')
                        assert 'Command shortcuts' in error, error
                        time.sleep(.1)
                        assert not any(e.get('seq', 0) > seq and e.get('type') in ('keydown','keyup','input') for e in lab.events)
                        report['checks'].append('Native background Command shortcut rejected without input')
                    if not chrome_devtools:
                        seq = max((e.get('seq', 0) for e in lab.events), default=0)
                        error = call('virtual_keyboard', expect_error=True, setup_mode=mode, **target, action='type', text='must-not-partially-insert 🙂')
                        assert 'supplementary Unicode' in error, error
                        time.sleep(.1)
                        assert not any(e.get('seq', 0) > seq and e.get('type') in ('keydown','keyup','input') for e in lab.events)
                        report['checks'].append('Native supplementary Unicode rejected before partial insertion: ' + mode)
                    marker = ('MCP café 测试 🙂 ' if chrome_devtools else 'MCP café 测试 ') + mode
                    events = chrome_action('Unicode typing', mode, 'type', text=marker)
                    assert any(e.get('type') == 'input' and e.get('value') == prefix + marker + suffix for e in events), events
                    events = chrome_action('backspace', mode, 'key', key='backspace')
                    assert any(e.get('type') == 'input' and e.get('value') == prefix + marker[:-1] + suffix for e in events), events
                    events = chrome_action('targeted scrolling', mode, 'scroll', control='scrollbox', delta=3)
                    assert any(e.get('type') == 'scrolled' and e.get('target') == 'scrollbox' for e in events), events
                    assert not any(e.get('type') == 'scrolled' and e.get('target') == 'scrollbox2' for e in events), events
                    report['checks'].append('Chrome ' + mode + ' recipient-verified pointer, keyboard, Unicode, scroll; drag=' + ('verified' if chrome_devtools else 'safely rejected'))
                call('virtual_pointer', expect_error=True, **{**target, 'pid': backend.pid}, action='click', x=10, y=10)
                call('virtual_pointer', expect_error=True, **target, action='click', x=-1, y=0)
                report['checks'].append('Chrome wrong-owner and out-of-bounds input rejected')

            if 'blender' in apps:
                if call('list_windows')['desktop']['frontmost_pid'] != original_front:
                    activate(original_front)
                statefile = temp / 'blender-state.json'
                fixture = ROOT / 'tests/fixtures/blender_input_probe.py'
                command = ['/usr/bin/open', '-g', '-n', '-a', 'Blender', '--args', '--factory-startup',
                           '--no-window-focus', '--window-geometry', '120', '100', '1000', '600',
                           '--python', str(fixture), '--', str(statefile)]
                call('bash', command=shlex.join(command), wait_ms=10000)
                def probe(): return json.loads(statefile.read_text())
                observed = wait_for(probe, 'Blender observer readiness', 35)
                pid = observed['pid']; pids.add(pid)
                def blender_window():
                    ws = [w for w in call('list_windows')['windows'] if w['pid'] == pid and w.get('width', 0) > 1]
                    return max(ws, key=lambda w:w['width']*w['height']) if ws else None
                win = wait_for(blender_window, 'exact Blender window')
                target = dict(window_id=win['window_id'], pid=pid, mode='background')
                assert call('list_windows')['desktop']['frontmost_pid'] == original_front, 'Blender launch stole focus'
                report['checks'].append('MCP Bash opened Blender background without touching user files or preferences')
                shot = call('get_screenshot', **target)

                def viewport():
                    p = probe(); w = p['windows'][0]
                    a = next(a for a in w['areas'] if a['type'] == 'VIEW_3D')
                    r = next(r for r in a['regions'] if r['type'] == 'WINDOW')
                    return p,w,a,r

                def blender_action(label, mode, action, *, xy=None, to=None, **kwargs):
                    nonlocal shot
                    prev = probe(); tool = 'virtual_keyboard' if action in {'key', 'type'} else 'virtual_pointer'
                    args = dict(target, action=action, **kwargs)
                    if tool == 'virtual_pointer':
                        _,w,_,r = viewport()
                        # Blender reports drawable coordinates from bottom-left;
                        # map them into full-window screenshot pixel coordinates.
                        def pixel(uv):
                            sx = shot['width'] / (w['width'] * w['pixel_size'])
                            sy = sx
                            return (round((r['x'] + r['width']*uv[0])*sx),
                                    round(shot['height'] - (r['y'] + r['height']*uv[1])*sy))
                        args['x'], args['y'] = pixel(xy or (.5,.5))
                        if to: args['to_x'], args['to_y'] = pixel(to)
                        args.update(screen_width=shot['width'], screen_height=shot['height'])
                    value = call(tool, setup_mode=mode, **args); shot = value
                    record('Blender ' + mode + ': ' + label, value, mode, target)
                    latest = wait_for(lambda: (p if (p:=probe())['time'] > prev['time'] else None), 'fresh Blender scene state')
                    report['actions'][-1]['recipient_events'] = [e for e in latest['events'] if e['seq'] > prev['seq']]
                    report['actions'][-1]['scene'] = {k:v for k,v in latest.items() if k != 'events'}
                    return latest

                for mode in ('background', 'foreground', 'background'):
                    activate(pid if mode == 'foreground' else original_front)
                    blender_action('viewport move', mode, 'move')
                    before = viewport()[2]['sidebar']
                    state = blender_action('toggle sidebar', mode, 'key', key='n')
                    assert viewport()[2]['sidebar'] != before, ('N did not toggle sidebar', state)
                    blender_action('restore sidebar', mode, 'key', key='n')
                    assert viewport()[2]['sidebar'] == before
                    distance = viewport()[2]['view_distance']
                    state = blender_action('viewport scroll', mode, 'scroll', delta=2)
                    assert abs(viewport()[2]['view_distance'] - distance) > .001, ('scroll did not zoom', state)
                    blender_action('deselect all', mode, 'key', key='alt+a')
                    assert not any(o['selected'] for o in probe()['objects']), 'deselect shortcut ignored'
                    blender_action('begin box selection', mode, 'key', key='b')
                    state = blender_action('box-selection drag', mode, 'drag', xy=(.15,.15), to=(.85,.85), duration=.5)
                    assert any(o['selected'] for o in state['objects']), ('drag did not select an object', state)
                    blender_action('select all', mode, 'key', key='a')
                    cube = next(o for o in probe()['objects'] if o['name'] == 'Cube')
                    old_x = cube['location'][0]
                    for key in ('g','x','2','enter'):
                        state = blender_action('transform: ' + key, mode, 'key', key=key)
                    cube = next(o for o in state['objects'] if o['name'] == 'Cube')
                    assert abs(cube['location'][0] - old_x - 2) < .01, ('transform did not commit', state)
                    count = len(state['objects'])
                    blender_action('open operator search', mode, 'key', key='f3')
                    blender_action('type operator name', mode, 'type', text='Add Cube')
                    state = blender_action('run typed operator', mode, 'key', key='enter')
                    assert len(state['objects']) == count + 1, ('typed operator did not create cube', state)
                    blender_action('delete test cube', mode, 'key', key='x')
                    state = blender_action('confirm deletion', mode, 'key', key='enter')
                    assert len(state['objects']) == count, ('deletion did not commit', state)
                    report['checks'].append('Blender ' + mode + ' scene-verified sidebar, scroll, box drag, transform, typing, deletion')
                call('virtual_keyboard', expect_error=True, **{**target, 'pid': backend.pid}, action='key', key='n')
                report['checks'].append('Blender wrong-owner input rejected')
            report['passed'] = True
        except Exception as error:
            report['error'] = repr(error)
            report['traceback'] = traceback.format_exc()
            raise
        finally:
            (report_dir / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False))
            if original_front:
                try:
                    front = call('list_windows')['desktop']['frontmost_pid']
                    if front in pids: activate(original_front)
                except Exception: pass
            clients.close()
            # Only processes belonging to this disposable test are stopped.
            try:
                for row in subprocess.check_output(['ps','-ax','-o','pid=','-o','command='],text=True).splitlines():
                    pidtext, _, cmd = row.strip().partition(' ')
                    if cmd.startswith('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome ') and f'--user-data-dir={temp}' in cmd:
                        pids.add(int(pidtext))
            except Exception: pass
            for pid in pids:
                try: os.kill(pid, signal.SIGTERM)
                except ProcessLookupError: pass
            for lab in labs: lab.shutdown(); lab.server_close()
            backend.terminate()
            try: backend.wait(timeout=8)
            except subprocess.TimeoutExpired: backend.kill(); backend.wait()
            log.close()
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--apps', nargs='+', choices=['chrome','blender'], default=['chrome','blender'])
    parser.add_argument('--report-dir', type=Path, default=Path('output/macos-app-input'))
    parser.add_argument('--chrome-devtools', action='store_true', help='Enable DevTools only on the disposable Chrome test profile')
    args = parser.parse_args()
    lockpath = Path(tempfile.gettempdir()) / ('lessagent-macos-input-matrix-' + str(os.getuid()) + '.lock')
    with lockpath.open('a') as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        outcome = run(args.binary.resolve(), args.apps, (ROOT / args.report_dir).resolve(), args.chrome_devtools)
    print(json.dumps({k:v for k,v in outcome.items() if k != 'actions'}, indent=2))
