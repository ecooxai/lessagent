#!/usr/bin/env python3
"""Verify detached startup, MCP readiness, and interactive TUI reconnection."""
import fcntl
import struct
import sys
import termios
import json
import os
import pathlib
import pty
import re
import select
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error

binary = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv)>1 else pathlib.Path(__file__).resolve().parents[1] / 'target/debug/lessagent'
with tempfile.TemporaryDirectory() as directory:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    args = [str(binary), '--data-dir', directory, '--port', str(port)]
    result = subprocess.run(args, capture_output=True, text=True, timeout=15)
    assert result.returncode == 0, result.stderr
    pid = int(re.search(r'Backend PID: (\d+)', result.stdout)[1])
    try:
        assert f'App HTTP port: {port}' in result.stdout
        assert f'MCP HTTP port: {port}' in result.stdout
        token = pathlib.Path(directory, 'token').read_text()
        def request(path, body=None):
            req = urllib.request.Request(f'http://127.0.0.1:{port}{path}',
                data=json.dumps(body).encode() if body else None,
                headers={'Authorization': f'Bearer {token}', 'Content-Type': 'application/json'})
            return json.load(urllib.request.urlopen(req, timeout=5))
        assert urllib.request.urlopen(f'http://127.0.0.1:{port}/api/state', timeout=5).status == 200
        assert 'result' in request('/mcp', {'jsonrpc':'2.0', 'id':1, 'method':'tools/list'})
        second = subprocess.run(args, capture_output=True, text=True, timeout=10)
        assert second.returncode == 0 and 'Connecting to running' in second.stdout
        assert 'Backend PID:' not in second.stdout
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
        ui = subprocess.Popen(args + ['tui'], stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        output = b''
        def until(text):
            global output
            deadline = time.monotonic() + 10
            while text not in output and time.monotonic() < deadline:
                if select.select([master], [], [], .2)[0]:
                    output += os.read(master, 65536)
            assert text in output, output[-2000:]
        try:
            until(b'No workspaces')
            os.write(master, f'/open {directory}\r'.encode())
            until(b'Workspace 1/1')
            os.write(master, b'\x1b[5~\x1b[6~')
            time.sleep(.3)
            os.write(master, b'\x03')
            deadline = time.monotonic() + 10
            while ui.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], .1)[0]:
                    try: os.read(master, 65536)
                    except OSError: break
            assert ui.wait(timeout=1) == 0
            assert len(request('/api/state')['workspaces']) == 1
        finally:
            if ui.poll() is None:
                ui.kill()
                ui.wait()
            os.close(master)
        # A plain interactive launch offers a choice instead of entering the TUI.
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
        ui = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        output = b''
        try:
            until(b'Open [c] CLI, [b] browser UI, or [n] neither?')
            os.write(master, b'n\n')
            assert ui.wait(timeout=5) == 0
        finally:
            if ui.poll() is None: ui.kill(); ui.wait()
            os.close(master)
        def cli(*command):
            r = subprocess.run(args + list(command), capture_output=True, text=True, timeout=20)
            assert r.returncode == 0, (command, r.stdout, r.stderr)
            return r.stdout
        def public(path, body=None, headers=None, expected=200):
            req = urllib.request.Request(f'http://127.0.0.1:{port}{path}',
                data=json.dumps(body).encode() if body is not None else None,
                headers={'Content-Type':'application/json', **(headers or {})})
            try: response = urllib.request.urlopen(req, timeout=5)
            except urllib.error.HTTPError as error: response = error
            assert response.status == expected, (path, response.status)
            return json.load(response)
        public('/api/state', headers={'Origin':'https://evil.example'}, expected=403)
        public('/api/state', headers={'Sec-Fetch-Site':'cross-site'}, expected=403)
        remote={'Host':f'lessagent.test:{port}', 'Origin':f'http://lessagent.test:{port}'}
        public('/api/state', headers=remote, expected=401)
        public('/api/state', headers={**remote, 'Authorization':f'Bearer {token}'})
        folders=pathlib.Path(directory,'folders'); folders.mkdir()
        for name in ['.config','Documents','Downloads','project']:
            (folders/name).mkdir()
        listing=request('/api/action/browse', {'path':str(folders)})
        assert {e['name'] for e in listing['entries'] if e['directory']} == {'.config','Documents','Downloads','project'}
        second_workspace=request('/api/action/workspace_open',{'path':str(folders)})
        request('/api/action/ui', {'ui':{'selected':second_workspace['id']}})
        public('/api/admin/shutdown', {}, expected=401)
        public('/mcp', {'jsonrpc':'2.0','id':1,'method':'tools/list'})
        cli('start', '--passwd', 'test password')
        public('/api/state', expected=401)
        public('/api/login', {'password':'wrong'}, expected=401)
        assert public('/api/login', {'password':'test password'}, headers=remote)['token'] == token
        assert 'test password' not in pathlib.Path(directory, 'password.hash').read_text()
        restarted = cli('restart')
        pid = int(re.search(r'Backend PID: (\d+)', restarted)[1])
        public('/api/state', expected=401)
        assert public('/api/login', {'password':'test password'}, headers=remote)['token'] == token
        restored=request('/api/state')
        assert len(restored['workspaces']) == 2
        assert restored['ui']['selected'] == second_workspace['id']
        assert any(w['path'] == str(folders.resolve()) for w in restored['workspaces'])
        assert 'stopped' in cli('stop')
        pid = None
        assert 'already stopped' in cli('stop')
        restarted = cli('restart', '--passwd', 'new password')
        pid = int(re.search(r'Backend PID: (\d+)', restarted)[1])
        public('/api/login', {'password':'test password'}, expected=401)
        assert public('/api/login', {'password':'new password'})['token'] == token
        assert 'stopped' in cli('stop')
        pid = None
        print('PASS: startup/menu/TUI, local token-free access, password login/persistence/change, public MCP access, stop/restart')
    finally:
        if pid is not None:
            os.kill(pid, signal.SIGINT)
            time.sleep(.5)
