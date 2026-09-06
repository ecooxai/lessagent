#!/usr/bin/env python3
"""Exercise upgrades from a backend without administration endpoints (requires lsof)."""
import json
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lessagent').resolve()
if not shutil.which('lsof'):
    raise SystemExit('SKIP: lsof is required for legacy backend detection')
fixture = r'''
import fcntl, http.server, json, pathlib, sys
root=pathlib.Path(sys.argv[1]); port=int(sys.argv[2])
lock=(root/'server.lock').open('w'); fcntl.flock(lock,fcntl.LOCK_EX)
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path=='/health': data={'service':'lessagent','ok':True}
        elif self.path=='/api/state' and self.headers.get('Authorization')=='Bearer test-token': data={'workspaces':[],'settings':{}}
        else: self.send_error(401); return
        self.send_response(200); self.send_header('Content-Type','application/json'); self.end_headers(); self.wfile.write(json.dumps(data).encode())
    def do_POST(self): self.send_error(404)
    def log_message(self,*args): pass
server=http.server.HTTPServer(('127.0.0.1',port),Handler)
try: server.serve_forever()
except KeyboardInterrupt: pass
finally: server.server_close(); lock.close()
'''
with tempfile.TemporaryDirectory() as directory:
    root=pathlib.Path(directory); (root/'token').write_text('test-token')
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
    args=[str(binary),'--port',str(port),'--data-dir',directory]
    for operation in [('stop',),('restart','--passwd','migration password'),('--passwd','migration password')]:
        old=subprocess.Popen([sys.executable,'-c',fixture,directory,str(port)])
        try:
            for _ in range(100):
                try:
                    urllib.request.urlopen(f'http://127.0.0.1:{port}/health',timeout=.2); break
                except OSError: time.sleep(.05)
            result=subprocess.run(args+list(operation),capture_output=True,text=True,timeout=20)
            assert result.returncode==0,(result.stdout,result.stderr)
            assert 'Stopping older Lessagent backend' in result.stdout
            old.wait(timeout=5)
            if operation[0]!='stop':
                req=urllib.request.Request(f'http://127.0.0.1:{port}/api/login',data=json.dumps({'password':'migration password'}).encode(),headers={'Content-Type':'application/json'})
                assert json.load(urllib.request.urlopen(req))['token']=='test-token'
        finally:
            if old.poll() is None: old.terminate(); old.wait()
            subprocess.run(args+['stop'],capture_output=True,timeout=15)
print('PASS: legacy stop, restart, and password-only launch with custom port/data directory')
