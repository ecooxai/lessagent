#!/usr/bin/env python3
"""macOS browser sizing and image metadata; uses only a debug backend and local page.
uv run --with 'mcp>=1.20,<2' tests/browser_geometry.py target/debug/lessagent
"""
import asyncio
import base64
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import urllib.request

from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
URI = 'lessagent://server/instruction.md'
REPORT = Path(os.environ.get('LESSAGENT_GEOMETRY_REPORT_DIR', 'output/browser-geometry'))


def payload(result):
    assert not result.isError, result
    return result.structuredContent['result']


def image_geometry(result):
    value = payload(result)
    image = next(c for c in result.content if c.type == 'image')
    raw = base64.b64decode(image.data)
    assert raw[:8] == b'\x89PNG\r\n\x1a\n'
    width, height = struct.unpack('>II', raw[16:24])
    assert (value['width'], value['height']) == (width, height)
    assert (value['screen_width'], value['screen_height']) == (width, height)
    wire = image.model_dump(by_alias=True, exclude_none=True)
    assert (wire['width'], wire['height']) == (width, height)
    assert wire['_meta']['lessagent/image'] == value['image_metadata']
    assert wire['metadata']['format'] == 'png'
    assert wire['metadata']['coordinate_space'] == 'window'
    assert Path(value['path']).read_bytes() == raw
    return width, height


async def exercise(root):
    work = root / 'project'; work.mkdir()
    (work / 'index.html').write_text('<!doctype html><title>Lessagent geometry fixture</title><h1>Local browser geometry fixture</h1><input aria-label="Test input">')
    class Quiet(SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass
    page = ThreadingHTTPServer(('127.0.0.1', 0), partial(Quiet, directory=str(work)))
    page_thread = threading.Thread(target=page.serve_forever, daemon=True); page_thread.start()
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]
    assert port != 3210
    base = f'http://127.0.0.1:{port}'
    data = root / 'data'
    log = (root / 'server.log').open('w')
    server = subprocess.Popen([str(BINARY), 'serve', '--port', str(port), '--data-dir', str(data)], stdout=log, stderr=log)
    pids = set()
    report = {'passed': False, 'binary': str(BINARY), 'port': port, 'checks': []}
    def api(path, body=None):
        request = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)
    def check(text):
        report['checks'].append(text); print('PASS:', text, flush=True)
    try:
        for _ in range(300):
            try:
                state = api('/api/state'); break
            except OSError:
                if server.poll() is not None: raise AssertionError('Debug server exited')
                await asyncio.sleep(.05)
        else: raise AssertionError('Debug server not ready')
        async with streamablehttp_client(base + '/mcp') as (read, write, _):
            async with ClientSession(read, write) as client:
                await client.initialize()
                first = (await client.read_resource(URI)).contents[0]
                host = first.model_dump(by_alias=True)['_meta']['lessagent/system']
                assert not host['computer_control_enabled']
                assert host['permissions']['accessibility'] and host['permissions']['screen_recording'], 'Enable macOS Accessibility and Screen Recording for this test'
                assert host['cpu']['model'] != 'Unavailable' and host['ram']['total_bytes'] > 0 and host['gpus']
                primary = next(display for display in host['displays'] if display['primary'])
                maxw, maxh = primary['logical_width'], primary['logical_height']
                definitions = {tool.name: tool for tool in (await client.list_tools()).tools}
                for name in ['browser_open', 'computer']:
                    fields = definitions[name].inputSchema['properties']
                    assert fields['width']['maximum'] == maxw and fields['height']['maximum'] == maxh
                REPORT.mkdir(parents=True, exist_ok=True)
                (REPORT / 'instruction-sample.md').write_text(first.text)
                (REPORT / 'system-info.json').write_text(json.dumps(host, indent=2))
                check('resource is readable while computer control is disabled; CPU/GPU/RAM and primary display are real')
                state['settings']['computer_enabled'] = True
                api('/api/action/settings', state['settings'])
                refreshed = (await client.read_resource(URI)).contents[0].model_dump(by_alias=True)['_meta']['lessagent/system']
                assert refreshed['computer_control_enabled'] and refreshed['observed_at_unix_ms'] >= host['observed_at_unix_ms']
                opened = await client.call_tool('workspace_open', {'path': str(work), 'summary': 'Open a disposable browser-geometry fixture workspace.'})
                workspace = payload(opened)['id']
                async def call(name, **args):
                    return await client.call_tool(name, {'workspace': workspace, 'summary': f'Verify {name} sizing and screenshot metadata on the local fixture.', **args})
                url = f'http://127.0.0.1:{page.server_port}/'
                for name in ['browser_open', 'computer']:
                    action = {'action': 'browser_open'} if name == 'computer' else {}
                    for size in [dict(width=maxw+1), dict(height=maxh+1), dict(width=639), dict(height=479),
                                 dict(width=True), dict(height=600.5), dict(width=-1), dict(width=2**63)]:
                        rejected = await call(name, url=url, **action, **size)
                        assert rejected.isError, (name, size, rejected)
                assert not list((data / 'browsers').glob('profile-*')), 'Invalid sizes created a browser profile'
                check('both browser entrypoints reject 16 invalid/stale sizes before creating profiles or windows')
                for label, name, sizes in [('default', 'browser_open', {}), ('maximum', 'computer', {'action': 'browser_open', 'width': maxw, 'height': maxh})]:
                    result = await call(name, url=url, **sizes)
                    value = payload(result); pids.add(value['pid'])
                    width, height = image_geometry(result)
                    size = value['browser_size']
                    requested = (min(1000, maxw), min(600, maxh)) if label == 'default' else (maxw, maxh)
                    assert (size['requested_width'], size['requested_height']) == requested
                    assert (size['width'], size['height']) == (min(requested[0], primary['visible_width']), min(requested[1], primary['visible_height']))
                    assert (value['logical_width'], value['logical_height']) == (size['width'], size['height'])
                    assert width > 0 and height > 0 and size['units'] == 'logical_points'
                    assert Path(value['path']).resolve().is_relative_to((work / 'output' / 'computer').resolve()), (value['path'], str(work))
                    for key in ['before', 'after', 'desktop']:
                        assert value[key]['frontmost_pid'] != value['pid'], (label, key, value)
                    shutil.copy2(value['path'], REPORT / (label + '.png'))
                    report[label] = {'browser_size': size, 'image_width': width, 'image_height': height}
                    target = {'window_id': value['window_id'], 'pid': value['pid']}
                    shot = await call('computer', action='screenshot', **target)
                    image_geometry(shot)
                    api_shot = api('/api/action/tool', {'workspace': workspace, 'name': 'computer', 'arguments': {'action': 'screenshot', **target}})
                    assert 'image' not in api_shot
                    actual = struct.unpack('>II', Path(api_shot['path']).read_bytes()[16:24])
                    assert (api_shot['width'], api_shot['height']) == actual
                    assert api_shot['image_metadata']['width'] == actual[0]
                    reread = await call('read_file', path=str(Path(value['path']).resolve().relative_to(work.resolve())))
                    read_value = payload(reread)
                    assert (read_value['width'], read_value['height']) == (width, height)
                    check(f'{label}: fitted browser size, output/ screenshots, encoded dimensions and metadata across MCP/API/file reads')
                report['passed'] = True
    finally:
        # Only the disposable Chrome processes launched by this test are stopped.
        for pid in pids:
            try: os.kill(pid, signal.SIGTERM)
            except ProcessLookupError: continue
            for _ in range(100):
                try: os.kill(pid, 0)
                except ProcessLookupError: break
                await asyncio.sleep(.05)
            else:
                try: os.kill(pid, signal.SIGKILL)
                except ProcessLookupError: pass
        if server.poll() is None:
            server.send_signal(signal.SIGINT)
            try: server.wait(timeout=10)
            except subprocess.TimeoutExpired: server.kill(); server.wait()
        page.shutdown(); page.server_close(); page_thread.join(timeout=5); log.close()
        REPORT.mkdir(parents=True, exist_ok=True)
        (REPORT / 'report.json').write_text(json.dumps(report, indent=2))
        shutil.copy2(root / 'server.log', REPORT / 'server.log')


if __name__ == '__main__':
    assert sys.platform == 'darwin', 'This integration test requires macOS'
    with tempfile.TemporaryDirectory(prefix='lessagent-browser-geometry-') as tmp:
        asyncio.run(exercise(Path(tmp)))
