#!/usr/bin/env python3
"""Focused macOS regression for managed Chrome profile reuse + standalone virtual input.

Usage:
  uv run --with 'mcp>=1.20,<2' tests/browser_profile_virtual_tools.py target/debug/lessagent

Runs only against a disposable debug backend/data directory and local HTTP fixture.
Requires Google Chrome, Accessibility, and Screen Recording permissions.
"""
from contextlib import ExitStack
from anyio.from_thread import start_blocking_portal
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client
import base64
import json
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
import urllib.request

from control_lab import LabServer

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
REPORT = ROOT / os.environ.get(
    'LESSAGENT_PROFILE_TOOLS_REPORT_DIR',
    'output/browser-profile-virtual-tools',
)


def wait_until(predicate, message, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError(message)


def exercise():
    assert sys.platform == 'darwin', 'This integration test requires macOS'
    REPORT.mkdir(parents=True, exist_ok=True)
    result_report = {'passed': False, 'binary': str(BINARY), 'checks': []}
    with tempfile.TemporaryDirectory(prefix='lessagent-profile-tools-', ignore_cleanup_errors=True) as directory:
        temp = Path(directory)
        data = temp / 'data'
        legacy_profile = data / 'browsers' / 'profile-existing-test'
        legacy_profile.mkdir(parents=True, mode=0o700)
        (legacy_profile / 'ExistingProfileMarker').write_text('reuse this managed profile')
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        drawing = LabServer(('127.0.0.1', 0))
        drawing_port = drawing.server_address[1]
        records = drawing.events
        drawing_thread = threading.Thread(target=drawing.serve_forever, daemon=True)
        drawing_thread.start()
        log = (temp / 'server.log').open('w')
        server = subprocess.Popen(
            [str(BINARY), 'serve', '--port', str(port), '--data-dir', str(data)],
            stdout=log,
            stderr=log,
        )
        chrome_pid = None
        clients = ExitStack()
        try:
            def request(path, body=None):
                encoded = None if body is None else json.dumps(body).encode()
                req = urllib.request.Request(
                    f'http://127.0.0.1:{port}{path}',
                    data=encoded,
                    headers={'Content-Type': 'application/json'},
                )
                with urllib.request.urlopen(req, timeout=45) as response:
                    return json.load(response)

            def backend_state():
                if server.poll() is not None:
                    raise AssertionError('Debug backend exited before becoming ready')
                try:
                    return request('/api/state')
                except OSError:
                    return None

            state = wait_until(backend_state, 'Debug backend did not start')
            assert state['settings']['computer_enabled'] is False
            state['settings']['computer_enabled'] = True
            request('/api/action/settings', state['settings'])
            workspace = request('/api/action/workspace_open', {'path': str(ROOT)})['id']

            portal = clients.enter_context(start_blocking_portal())
            streams = clients.enter_context(portal.wrap_async_context_manager(streamablehttp_client(
                f'http://127.0.0.1:{port}/mcp',
                headers={'Authorization': 'Bearer ' + (data / 'token').read_text().strip()},
            )))
            client = clients.enter_context(portal.wrap_async_context_manager(ClientSession(streams[0], streams[1])))
            portal.call(client.initialize)
            definitions = {tool.name: tool for tool in portal.call(client.list_tools).tools}
            assert 'virtual_pointer' in definitions and 'virtual_keyboard' in definitions
            assert 'pointer' not in definitions and 'keyboard' not in definitions
            assert 'computer' not in definitions
            browser = definitions['browser_open']
            assert '?purpose=texttodescribepurposeofthiswindow_by_modelname' in browser.inputSchema['properties']['url']['description']
            assert 'existing persistent managed profile by default' in browser.description
            result_report['checks'].append('browser_open advertises existing-profile default and model-authored purpose query')
            result_report['checks'].append('renamed MCP tool surface')

            capture_index = 0
            report_relative = REPORT.resolve().relative_to(ROOT.resolve())

            def call(name, **args):
                nonlocal capture_index
                if name in {'browser_open', 'virtual_pointer', 'virtual_keyboard'} and 'capture_path' not in args:
                    capture_index += 1
                    args['capture_path'] = str(report_relative / f'observation-{capture_index:02d}.png')
                result = portal.call(
                    client.call_tool,
                    name,
                    dict(
                        workspace=workspace,
                        summary=f'Focused managed Chrome test for {name}.',
                        **args,
                    ),
                )
                assert not result.isError, result
                value = result.structuredContent['result']
                return result, value

            def assert_image(result, value):
                images = [block for block in result.content if block.type == 'image']
                assert len(images) == 1, (value, result.content)
                image = images[0]
                assert image.mimeType == 'image/png'
                raw = base64.b64decode(image.data)
                assert raw[:8] == b'\x89PNG\r\n\x1a\n'
                actual = struct.unpack('>II', raw[16:24])
                assert actual == (value['screen_width'], value['screen_height'])
                assert actual == (value['width'], value['height'])
                assert raw == Path(value['path']).read_bytes()
                wire = image.model_dump(by_alias=True, exclude_none=True)
                assert (wire['width'], wire['height']) == actual
                assert wire['_meta']['lessagent/image'] == value['image_metadata']
                return actual

            url = f'http://127.0.0.1:{drawing_port}/?purpose=verify_managed_profile_reuse_by_gpt-5.6-sol'
            first_result, first = call('browser_open', url=url, width=1000, height=600)
            assert_image(first_result, first)
            chrome_pid = first['pid']
            assert first['url'] == url
            assert first['persistent_profile'] and first['isolated_profile']
            assert first['profile_reused'] and not first['browser_process_reused']
            assert (legacy_profile / 'ExistingProfileMarker').read_text() == 'reuse this managed profile'
            assert (legacy_profile / 'DevToolsActivePort').is_file()
            assert not (data / 'browsers' / 'profile').exists()
            assert first['after']['frontmost_pid'] != chrome_pid
            result_report['checks'].append('first browser_open adopted an existing legacy managed profile instead of creating a blank one')
            ready1 = wait_until(
                lambda: next((e for e in reversed(records) if e['type'] == 'ready'), None),
                'First managed Chrome window did not become ready',
            )
            top1 = first['logical_height'] - ready1['innerHeight']

            def screenshot_point(value, ready, top, element):
                box = ready['geometry'][element]
                logical_x = round(box['x'] + box['width'] / 2)
                logical_y = round(top + box['y'] + box['height'] / 2)
                return (
                    round(logical_x * value['screen_width'] / value['logical_width']),
                    round(logical_y * value['screen_height'] / value['logical_height']),
                )

            first_target = dict(window_id=first['window_id'], pid=first['pid'], mode='background')
            x, y = screenshot_point(first, ready1, top1, 'name')
            pointer_result, pointer_value = call(
                'virtual_pointer',
                action='click', x=x, y=y,
                screen_width=first['screen_width'], screen_height=first['screen_height'],
                **first_target,
            )
            assert_image(pointer_result, pointer_value)
            assert pointer_value['automatic_screenshot'] and pointer_value['delivery'] == 'chrome-devtools'

            key_result, key_value = call('virtual_keyboard', action='key', key='cmd+a', **first_target)
            assert_image(key_result, key_value)
            probe = 'Persistent profile via virtual keyboard ✓'
            type_result, type_value = call('virtual_keyboard', action='type', text=probe, **first_target)
            assert_image(type_result, type_value)
            assert type_value['automatic_screenshot'] and type_value['delivery'] == 'chrome-devtools'
            wait_until(
                lambda: next((e for e in reversed(records) if e['type'] == 'input' and e.get('value') == probe), None),
                'virtual_keyboard input was not observed by the real page',
            )
            result_report['checks'].append('virtual_pointer and virtual_keyboard each returned fresh MCP PNG images')

            second_record_start = len(records)
            second_result, second = call('browser_open', url=url, width=1000, height=600)
            assert_image(second_result, second)
            assert second['url'] == url
            assert second['persistent_profile'] and second['profile_reused'] and second['browser_process_reused']
            assert second['pid'] == first['pid']
            assert second['window_id'] != first['window_id']
            assert second['after']['frontmost_pid'] != chrome_pid
            ready2 = wait_until(
                lambda: next((
                    e for e in reversed(records[second_record_start:])
                    if e['type'] == 'ready' and e.get('profileValue') == probe
                ), None),
                'Second browser_open window did not inherit localStorage from the persistent managed profile',
            )
            result_report['checks'].append('second browser_open reused profile/process and inherited localStorage')

            second_target = dict(window_id=second['window_id'], pid=second['pid'], mode='background')
            x2, y2 = screenshot_point(second, ready2, second['logical_height'] - ready2['innerHeight'], 'name')
            second_pointer_result, second_pointer = call(
                'virtual_pointer',
                action='click', x=x2, y=y2,
                screen_width=second['screen_width'], screen_height=second['screen_height'],
                **second_target,
            )
            assert_image(second_pointer_result, second_pointer)
            second_key_result, second_key = call('virtual_keyboard', action='key', key='cmd+a', **second_target)
            assert_image(second_key_result, second_key)
            second_probe = 'Second window input ✓'
            second_type_result, second_type = call('virtual_keyboard', action='type', text=second_probe, **second_target)
            assert_image(second_type_result, second_type)
            wait_until(
                lambda: next((e for e in reversed(records) if e['type'] == 'input' and e.get('value') == second_probe), None),
                'Renamed virtual tools did not control the newly created second Chrome window',
            )
            result_report['checks'].append('renamed virtual tools controlled the second window sharing the same Chrome PID')

            result_report.update(
                passed=True,
                first={'pid': first['pid'], 'window_id': first['window_id']},
                second={'pid': second['pid'], 'window_id': second['window_id']},
            )
            print(json.dumps(result_report, indent=2), flush=True)
        finally:
            clients.close()
            if chrome_pid:
                try:
                    os.kill(chrome_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                for _ in range(100):
                    try:
                        os.kill(chrome_pid, 0)
                    except ProcessLookupError:
                        break
                    time.sleep(.05)
                else:
                    try:
                        os.kill(chrome_pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            if server.poll() is None:
                server.send_signal(signal.SIGINT)
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
            drawing.shutdown()
            drawing.server_close()
            drawing_thread.join(timeout=5)
            log.close()
            (REPORT / 'report.json').write_text(json.dumps(result_report, indent=2))
            (REPORT / 'server.log').write_text((temp / 'server.log').read_text())


if __name__ == '__main__':
    exercise()
