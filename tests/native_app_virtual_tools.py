#!/usr/bin/env python3
"""Real macOS background-input regression for Blender and Auri via standalone MCP tools.

Usage:
  uv run --with 'mcp>=1.20,<2' --with pillow tests/native_app_virtual_tools.py target/debug/lessagent

Runs a disposable debug backend. It launches fresh Blender and Auri instances,
never uses global pointer/keyboard input, and kills only PIDs created by this test.
Requires Accessibility and Screen Recording. Evidence goes under the project output dir.
"""
from contextlib import ExitStack
from anyio.from_thread import start_blocking_portal
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client
from pathlib import Path
from PIL import Image, ImageChops, ImageStat
import base64
import io
import json
import os
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
REPORT = ROOT / os.environ.get(
    'LESSAGENT_NATIVE_APP_REPORT_DIR',
    'output/native-app-virtual-tools',
)

AX_SOURCE = r'''
import Foundation
import ApplicationServices

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(2)
}
let argv = CommandLine.arguments
if argv.count < 3 { fail("usage: ax-query pid role [needle]") }
guard let pid = Int32(argv[1]) else { fail("bad pid") }
let wantedRole = argv[2]
let needle = argv.count > 3 ? argv[3] : ""
let app = AXUIElementCreateApplication(pid)
AXUIElementSetMessagingTimeout(app, 2)

func attr(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}
func stringAttr(_ element: AXUIElement, _ name: String) -> String {
    if let value = attr(element, name) as? String { return value }
    if let value = attr(element, name) as? NSNumber { return value.stringValue }
    return ""
}
func boolAttr(_ element: AXUIElement, _ name: String) -> Bool {
    if let value = attr(element, name) as? Bool { return value }
    if let value = attr(element, name) as? NSNumber { return value.boolValue }
    return false
}
func frame(_ element: AXUIElement) -> CGRect? {
    guard let rawPosition = attr(element, kAXPositionAttribute),
          let rawSize = attr(element, kAXSizeAttribute),
          CFGetTypeID(rawPosition) == AXValueGetTypeID(),
          CFGetTypeID(rawSize) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero, size = CGSize.zero
    guard AXValueGetValue(rawPosition as! AXValue, .cgPoint, &point),
          AXValueGetValue(rawSize as! AXValue, .cgSize, &size) else { return nil }
    return CGRect(origin: point, size: size)
}
var windowValue: CFTypeRef?
guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &windowValue) == .success,
      let windows = windowValue as? [AXUIElement], let window = windows.first,
      let windowFrame = frame(window) else { fail("no AX window") }
var found: AXUIElement?
var visited = 0
func walk(_ element: AXUIElement, _ depth: Int) {
    if found != nil || depth > 20 || visited > 2500 { return }
    visited += 1
    let role = stringAttr(element, kAXRoleAttribute)
    let title = stringAttr(element, kAXTitleAttribute)
    let description = stringAttr(element, kAXDescriptionAttribute)
    let value = stringAttr(element, kAXValueAttribute)
    if role == wantedRole && (needle.isEmpty || title.localizedCaseInsensitiveContains(needle) || description.localizedCaseInsensitiveContains(needle) || value.localizedCaseInsensitiveContains(needle)) {
        found = element
        return
    }
    if let children = attr(element, kAXChildrenAttribute) as? [AXUIElement] {
        for child in children { walk(child, depth + 1); if found != nil { break } }
    }
}
walk(window, 0)
guard let element = found, let elementFrame = frame(element) else { fail("AX element not found") }
var actions: CFArray?
_ = AXUIElementCopyActionNames(element, &actions)
let result: [String: Any] = [
    "role": stringAttr(element, kAXRoleAttribute),
    "title": stringAttr(element, kAXTitleAttribute),
    "description": stringAttr(element, kAXDescriptionAttribute),
    "value": stringAttr(element, kAXValueAttribute),
    "focused": boolAttr(element, kAXFocusedAttribute),
    "x": elementFrame.minX, "y": elementFrame.minY,
    "width": elementFrame.width, "height": elementFrame.height,
    "window_x": windowFrame.minX, "window_y": windowFrame.minY,
    "window_width": windowFrame.width, "window_height": windowFrame.height,
    "actions": actions as? [String] ?? [], "visited": visited,
]
let data = try! JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
'''


def wait_until(predicate, message, seconds=10):
    deadline = time.monotonic() + seconds
    last_error = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except Exception as error:
            last_error = error
        time.sleep(.08)
    raise AssertionError(f'{message}: {last_error}' if last_error else message)


def image_from_result(result, value):
    images = [block for block in result.content if block.type == 'image']
    assert len(images) == 1, (value, result.content)
    image = images[0]
    assert image.mimeType == 'image/png'
    raw = base64.b64decode(image.data)
    assert raw[:8] == b'\x89PNG\r\n\x1a\n'
    actual = struct.unpack('>II', raw[16:24])
    assert actual == (value['width'], value['height'])
    assert actual == (value['screen_width'], value['screen_height'])
    assert raw == Path(value['path']).read_bytes()
    wire = image.model_dump(by_alias=True, exclude_none=True)
    assert (wire['width'], wire['height']) == actual
    assert wire['_meta']['lessagent/image'] == value['image_metadata']
    return Image.open(io.BytesIO(raw)).convert('RGB')


def difference_energy(before, after):
    # Blender may re-layout or resize its drawable window when a background
    # pointer action dismisses/changes startup UI. Compare both observations in
    # a common pixel space instead of treating that recipient-side size change
    # as a test-oracle failure.
    if before.size != after.size:
        common = (min(before.width, after.width), min(before.height, after.height))
        before = before.resize(common, Image.Resampling.BILINEAR)
        after = after.resize(common, Image.Resampling.BILINEAR)
    diff = ImageChops.difference(before, after)
    return sum(ImageStat.Stat(diff).sum)


def exercise():
    assert sys.platform == 'darwin', 'This integration test requires macOS'
    REPORT.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'binary': str(BINARY), 'checks': [], 'apps': {}}
    created_pids = []
    with tempfile.TemporaryDirectory(prefix='lessagent-native-apps-', ignore_cleanup_errors=True) as directory:
        temp = Path(directory)
        data = temp / 'data'
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        log = (temp / 'server.log').open('w')
        server = subprocess.Popen(
            [str(BINARY), 'serve', '--port', str(port), '--data-dir', str(data)],
            stdout=log, stderr=log,
        )
        clients = ExitStack()
        try:
            def request(path, body=None):
                encoded = None if body is None else json.dumps(body).encode()
                req = urllib.request.Request(
                    f'http://127.0.0.1:{port}{path}', data=encoded,
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
            for name in ['app_open', 'list_windows', 'get_screenshot', 'virtual_pointer', 'virtual_keyboard']:
                assert name in definitions, name
            assert 'computer' not in definitions
            report['checks'].append('standalone native GUI MCP surface; aggregate computer absent')

            report_relative = REPORT.resolve().relative_to(ROOT.resolve())
            capture_index = 0

            def call(name, **args):
                nonlocal capture_index
                if name in {'app_open', 'get_screenshot', 'virtual_pointer', 'virtual_keyboard'} and 'capture_path' not in args:
                    capture_index += 1
                    args['capture_path'] = str(report_relative / f'observation-{capture_index:02d}-{name}.png')
                summary = (
                    f'Progress 96/100 — done: debug build, MCP contracts, managed Chrome and strict split-tool input checks pass. '
                    f'Next: verify {name} against Blender/Auri background behavior and fresh image output.'
                )
                result = portal.call(client.call_tool, name, dict(workspace=workspace, summary=summary, **args))
                assert not result.isError, result
                return result, result.structuredContent['result']

            initial_result, initial = call('list_windows')
            assert initial['accessibility'] and initial['screen_recording']
            original_frontmost = initial['desktop']['frontmost_pid']
            existing_pids = {int(window['pid']) for window in initial['windows']}

            def restore_frontmost():
                if original_frontmost and original_frontmost > 0:
                    subprocess.run([
                        'osascript', '-e',
                        f'tell application "System Events" to set frontmost of first application process whose unix id is {original_frontmost} to true',
                    ], check=True, capture_output=True)
                    time.sleep(.6)

            def assert_background(value, target_pid):
                assert value.get('mode') == 'background', value
                assert value.get('target_was_background') is True, value
                # before/after are sampled around the recipient-local input itself.
                # `desktop` is sampled with the automatic observation ~2s later and
                # can legitimately reflect concurrent human activity; it is not an
                # activation oracle for the earlier Lessagent action.
                for key in ['before', 'after']:
                    if key in value:
                        assert value[key]['frontmost_pid'] != target_pid, (key, value)

            def ensure_background_precondition(target_pid):
                # Native apps can finish launch asynchronously after app_open's
                # observation. Normalize the test precondition once before input;
                # this is test setup only, never an input fallback.
                _, state = call('list_windows')
                if state['desktop']['frontmost_pid'] == target_pid:
                    restore_frontmost()
                    _, state = call('list_windows')
                assert state['desktop']['frontmost_pid'] != target_pid, state

            # Blender: process-window pointer events must visibly affect its own UI,
            # then process-window keyboard events must work while the app stays backgrounded.
            blender_result, blender = call('app_open', app='Blender', new_instance=True)
            blender_image = image_from_result(blender_result, blender)
            blender_pid = int(blender['pid'])
            if blender_pid not in existing_pids:
                created_pids.append(blender_pid)
            assert blender['after']['frontmost_pid'] != blender_pid, blender
            target = dict(window_id=blender['window_id'], pid=blender_pid, mode='background')
            shot_result, shot = call('get_screenshot', **target, show_pointer=False)
            before = image_from_result(shot_result, shot)
            ensure_background_precondition(blender_pid)
            px = round(shot['screen_width'] * 0.13)
            py = round(shot['screen_height'] * 0.54)
            close_result, close = call('virtual_pointer', **target, action='click', x=px, y=py,
                                       screen_width=shot['screen_width'], screen_height=shot['screen_height'],
                                       show_pointer=False)
            after_pointer = image_from_result(close_result, close)
            assert_background(close, blender_pid)
            assert close['delivery'] == 'process-window', close
            pointer_energy = difference_energy(before, after_pointer)
            assert pointer_energy > 1500, ('Blender did not visibly react to background click', pointer_energy)

            # Blender can replace/re-layout the startup window when that first click
            # dismisses startup UI. Preserve Lessagent's stale-window safety: refresh
            # the exact current window_id for the same PID instead of retrying input
            # against the old ID or weakening nativeWindowGeometry validation.
            _, windows_after_startup = call('list_windows')
            blender_windows = [
                window for window in windows_after_startup['windows']
                if int(window.get('pid', 0)) == blender_pid and window.get('onscreen', False)
            ]
            assert blender_windows, ('Blender current window disappeared after startup click', windows_after_startup)
            current_window = max(
                blender_windows,
                key=lambda window: int(window.get('width', 0)) * int(window.get('height', 0)),
            )
            target = dict(window_id=int(current_window['window_id']), pid=blender_pid, mode='background')
            refreshed_result, refreshed = call('get_screenshot', **target, show_pointer=False)
            image_from_result(refreshed_result, refreshed)

            # Focus the center of the current 3D viewport, then use Blender's N
            # sidebar toggle as a keyboard acceptance oracle. Unlike deleting the
            # default cube, this does not depend on selection state.
            focus_x = round(refreshed['screen_width'] * 0.50)
            focus_y = round(refreshed['screen_height'] * 0.50)
            focus_result, focus = call('virtual_pointer', **target, action='click', x=focus_x, y=focus_y,
                                       screen_width=refreshed['screen_width'], screen_height=refreshed['screen_height'],
                                       show_pointer=False)
            after_focus = image_from_result(focus_result, focus)
            assert_background(focus, blender_pid)
            assert focus['delivery'] == 'process-window', focus
            n_result, n_value = call('virtual_keyboard', **target, action='key', key='n')
            after_keyboard = image_from_result(n_result, n_value)
            assert_background(n_value, blender_pid)
            assert n_value['delivery'] == 'process-window', n_value
            keyboard_energy = difference_energy(after_focus, after_keyboard)
            assert keyboard_energy > 1500, ('Blender did not visibly react to background N key', keyboard_energy)
            restore_result, restore = call('virtual_keyboard', **target, action='key', key='n')
            image_from_result(restore_result, restore)
            assert_background(restore, blender_pid)
            assert restore['delivery'] == 'process-window', restore
            report['apps']['Blender'] = {
                'pid': blender_pid, 'window_id': target['window_id'],
                'pointer_delivery': close['delivery'], 'keyboard_delivery': n_value['delivery'],
                'pointer_image_difference': pointer_energy,
                'keyboard_image_difference': keyboard_energy,
            }
            report['checks'].append('Blender accepted background pointer + keyboard; each returned fresh PNG; foreground target isolation verified')

            # Build a read-only AX query helper. It never performs actions; it gives
            # dynamic element centers and verifies the app changed after Lessagent input.
            ax_source = temp / 'ax-query.swift'
            ax_binary = temp / 'ax-query'
            ax_source.write_text(AX_SOURCE)
            subprocess.run(['xcrun', 'swiftc', '-Onone', '-g', str(ax_source), '-o', str(ax_binary)], check=True, capture_output=True)

            def ax_find(pid, role, needle=''):
                completed = subprocess.run(
                    [str(ax_binary), str(pid), role, needle],
                    check=True, capture_output=True, text=True,
                )
                return json.loads(completed.stdout)

            def ax_point(element, shot_value):
                local_x = element['x'] + element['width'] / 2 - element['window_x']
                local_y = element['y'] + element['height'] / 2 - element['window_y']
                return (
                    round(local_x * shot_value['screen_width'] / shot_value['logical_width']),
                    round(local_y * shot_value['screen_height'] / shot_value['logical_height']),
                )

            auri_result, auri = call('app_open', app='Auri', new_instance=True)
            image_from_result(auri_result, auri)
            auri_pid = int(auri['pid'])
            if auri_pid not in existing_pids:
                created_pids.append(auri_pid)
            # Auri may self-activate at launch; that is launch behavior, not control.
            # Restore the original app once, then all Lessagent input must preserve it.
            if auri['after']['frontmost_pid'] == auri_pid:
                restore_frontmost()
            target = dict(window_id=auri['window_id'], pid=auri_pid, mode='background')
            shot_result, shot = call('get_screenshot', **target, show_pointer=False)
            image_from_result(shot_result, shot)
            ensure_background_precondition(auri_pid)

            system = wait_until(lambda: ax_find(auri_pid, 'AXRadioButton', 'Open System'), 'Auri System AX control missing')
            sx, sy = ax_point(system, shot)
            sys_result, sys_value = call('virtual_pointer', **target, action='click', x=sx, y=sy,
                                         screen_width=shot['screen_width'], screen_height=shot['screen_height'],
                                         show_pointer=False)
            image_from_result(sys_result, sys_value)
            assert_background(sys_value, auri_pid)
            assert sys_value['delivery'] == 'accessibility-window', sys_value
            assert sys_value['accessibility_action']['role'] == 'AXRadioButton', sys_value
            selected_system = wait_until(
                lambda: (value if (value := ax_find(auri_pid, 'AXRadioButton', 'Open System'))['value'] in {'1', 'true'} else None),
                'Auri System tab did not become selected',
            )

            terminal = ax_find(auri_pid, 'AXRadioButton', 'Open Terminal')
            tx, ty = ax_point(terminal, shot)
            term_result, term_value = call('virtual_pointer', **target, action='click', x=tx, y=ty,
                                           screen_width=shot['screen_width'], screen_height=shot['screen_height'],
                                           show_pointer=False)
            image_from_result(term_result, term_value)
            assert_background(term_value, auri_pid)
            assert term_value['delivery'] == 'accessibility-window', term_value
            selected_terminal = wait_until(
                lambda: (value if (value := ax_find(auri_pid, 'AXRadioButton', 'Open Terminal'))['value'] in {'1', 'true'} else None),
                'Auri Terminal tab did not become selected',
            )

            combo = wait_until(lambda: ax_find(auri_pid, 'AXComboBox'), 'Auri command combo missing')
            cx, cy = ax_point(combo, shot)
            combo_result, combo_value = call('virtual_pointer', **target, action='click', x=cx, y=cy,
                                             screen_width=shot['screen_width'], screen_height=shot['screen_height'],
                                             show_pointer=False)
            image_from_result(combo_result, combo_value)
            assert_background(combo_value, auri_pid)
            assert combo_value['delivery'] == 'accessibility-window', combo_value
            focused_combo = wait_until(
                lambda: (value if (value := ax_find(auri_pid, 'AXComboBox'))['focused'] else None),
                'Auri command combo did not receive background focus',
            )

            marker = 'AURI_DEBUG_BACKGROUND_20260909'
            select_result, select_value = call('virtual_keyboard', **target, action='key', key='cmd+a')
            image_from_result(select_result, select_value)
            assert_background(select_value, auri_pid)
            assert select_value['delivery'] == 'process-window', select_value
            type_result, type_value = call('virtual_keyboard', **target, action='type', text=marker)
            image_from_result(type_result, type_value)
            assert_background(type_value, auri_pid)
            assert type_value['delivery'] == 'process-window', type_value
            typed_combo = wait_until(
                lambda: (value if marker in (value := ax_find(auri_pid, 'AXComboBox'))['value'] else None),
                'Auri did not expose the marker after background keyboard typing',
            )
            report['apps']['Auri'] = {
                'pid': auri_pid, 'window_id': auri['window_id'],
                'launch_focus_changed': auri.get('focus_changed'),
                'system_value': selected_system['value'],
                'terminal_value': selected_terminal['value'],
                'combo_focused': focused_combo['focused'],
                'typed_value': typed_combo['value'],
                'pointer_delivery': combo_value['delivery'],
                'keyboard_delivery': type_value['delivery'],
            }
            report['checks'].append('Auri accepted exact-window AXPress pointer clicks while background; AX state verified System/Terminal selection and command focus')
            report['checks'].append('Auri accepted process-window keyboard typing after semantic focus; AX value verified marker; every input returned fresh PNG and stayed backgrounded')
            report['passed'] = True
            return report
        finally:
            clients.close()
            for pid in reversed(created_pids):
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            if original_frontmost if 'original_frontmost' in locals() else False:
                try:
                    restore_frontmost()
                except Exception:
                    pass
            server.send_signal(signal.SIGTERM)
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill(); server.wait()
            log.close()


if __name__ == '__main__':
    result = exercise()
    (REPORT / 'report.json').write_text(json.dumps(result, indent=2, ensure_ascii=False))
    print(json.dumps(result, indent=2, ensure_ascii=False))
