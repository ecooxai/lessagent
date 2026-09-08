#!/usr/bin/env python3
"""Typing must remain responsive while the state server takes one second."""
import fcntl, json, os, pathlib, pty, select, struct, subprocess, sys, tempfile, termios, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
class Server(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_GET(self):
        if 'summary=true' in self.path:time.sleep(1)
        body={'management_api':1} if self.path=='/health' else {'workspaces':[],'settings':{},'jobs':[]}
        self.send_response(200);self.end_headers()
        try:self.wfile.write(json.dumps(body).encode())
        except BrokenPipeError:pass
server=ThreadingHTTPServer(('127.0.0.1',0),Server)
threading.Thread(target=server.serve_forever,daemon=True).start()
with tempfile.TemporaryDirectory() as directory:
    pathlib.Path(directory,'token').write_text('fixture')
    master,slave=pty.openpty()
    fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',24,100,0,0))
    proc=subprocess.Popen([str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/lessagent').resolve()),'--data-dir',directory,'--port',str(server.server_port),'tui'],stdin=slave,stdout=slave,stderr=slave)
    os.close(slave)
    def read_for(duration):
        result=b'';deadline=time.monotonic()+duration
        while time.monotonic()<deadline:
            if select.select([master],[],[],max(0,deadline-time.monotonic()))[0]:result+=os.read(master,65536)
        return result
    try:
        output=b'';deadline=time.monotonic()+5
        while b'No workspaces' not in output:
            output+=read_for(.05);assert time.monotonic()<deadline,output
        start=time.perf_counter();os.write(master,b'latency-check')
        output=b''
        while b'> latency-check' not in output:
            output+=read_for(.01);assert time.perf_counter()-start<.5,output
        elapsed=(time.perf_counter()-start)*1000
        read_for(1.6)
        assert not read_for(.6),'unchanged state repainted the TUI'
        os.write(master,b'\x1b[O');read_for(.1)
        os.write(master,b'!');assert not read_for(.4),'unfocused TUI painted'
        os.write(master,b'\x1b[I');output=read_for(.3)
        assert b'> latency-check!' in output,output
        os.write(master,b'\x03');proc.wait(timeout=3);assert proc.returncode==0
        print(f'PASS: TUI typing {elapsed:.1f}ms with 1000ms state latency; unchanged/focus-lost UI stays quiet and focus restores it')
    finally:
        if proc.poll() is None:proc.kill();proc.wait()
        os.close(master)
server.shutdown()
