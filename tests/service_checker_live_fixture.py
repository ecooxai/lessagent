#!/usr/bin/env python3
"""Run a disposable debug monitor fixture until stopped; no production state.
Use the printed fixture URL /fail or /pass to toggle health, /counts to inspect.
The runtime file contains only test process IDs, ports and temporary paths.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

ROOT=Path(__file__).resolve().parent.parent
BINARY=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/lessagent').resolve()
assert 'debug' in BINARY.parts, 'Use debug build only'
REPORT=ROOT/'output/monitor-alert-validation'
REPORT.mkdir(parents=True,exist_ok=True)
state={'failed':False,'counts':{}}
stop=threading.Event()
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*_): pass
    def do_GET(self):
        path=self.path.split('?',1)[0]
        state['counts'][path]=state['counts'].get(path,0)+1
        if path=='/fail': state['failed']=True
        elif path=='/pass': state['failed']=False
        status=503 if path=='/unstable' and state['failed'] else 200
        body=(json.dumps(state) if path in ('/counts','/fail','/pass') else ('{"ok":true}' if path=='/health' else ('fixture failure' if status==503 else 'OK\nfixture'))).encode()
        self.send_response(status);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
fixture=ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=fixture.serve_forever,daemon=True).start()
with tempfile.TemporaryDirectory(prefix='lessagent-monitor-alert-') as tmp:
    temp=Path(tmp);data=temp/'backend';monitor_data=temp/'monitor';monitor_data.mkdir()
    base=f'http://127.0.0.1:{fixture.server_port}'
    (monitor_data/'service-check.json').write_text(json.dumps({'interval_seconds':1,'custom_urls':[base+'/unstable'],'errors':[]}))
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    assert port!=3210
    logs=(REPORT/'backend.log').open('w');gui_log=(REPORT/'monitor.log').open('w')
    backend=subprocess.Popen([str(BINARY),'serve','--port',str(port),'--data-dir',str(data)],stdout=logs,stderr=logs)
    monitor=None
    def api(path,body=None):
        r=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
        with urllib.request.urlopen(r,timeout=30) as response:return json.load(response)
    try:
        for _ in range(200):
            try: settings=api('/api/state')['settings'];break
            except OSError: time.sleep(.1)
        else:raise RuntimeError('Debug backend did not start')
        settings['computer_enabled']=True;api('/api/action/settings',settings)
        workspace=api('/api/action/workspace_open',{'path':str(ROOT)})['id']
        monitor=subprocess.Popen([str(BINARY),'service-check','--data-dir',str(monitor_data),'--base-url',base],stdout=gui_log,stderr=gui_log)
        runtime={'backend_port':port,'backend_pid':backend.pid,'monitor_pid':monitor.pid,'workspace':workspace,'fixture_url':base,'monitor_data':str(monitor_data),'binary':str(BINARY)}
        (REPORT/'runtime.json').write_text(json.dumps(runtime,indent=2))
        print(json.dumps(runtime,indent=2),flush=True)
        signal.signal(signal.SIGINT,lambda *_:stop.set());signal.signal(signal.SIGTERM,lambda *_:stop.set())
        while not stop.wait(.25):
            if monitor.poll() is not None:raise RuntimeError('Monitor exited: '+str(monitor.returncode))
            if backend.poll() is not None:raise RuntimeError('Backend exited: '+str(backend.returncode))
    finally:
        if monitor and monitor.poll() is None:
            monitor.terminate()
            try:monitor.wait(timeout=10)
            except subprocess.TimeoutExpired:monitor.kill();monitor.wait()
        if backend.poll() is None:
            backend.send_signal(signal.SIGINT)
            try:backend.wait(timeout=10)
            except subprocess.TimeoutExpired:backend.terminate();backend.wait(timeout=10)
        fixture.shutdown();fixture.server_close();logs.close();gui_log.close()
        config=monitor_data/'service-check.json'
        if config.exists():(REPORT/'final-config.json').write_bytes(config.read_bytes())
