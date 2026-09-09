import json, pathlib, subprocess, tempfile, threading, time, collections, select
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
ROOT=pathlib.Path.cwd(); binary=str(ROOT/'output/control-fix/state-probe'); records=[]
page='''<!doctype html><title>Lessagent Native Probe</title><style>body{margin:0}input{position:absolute;left:150px;top:45px;width:250px}button{position:absolute;left:500px;top:45px}canvas{position:absolute;top:150px;touch-action:none}</style><input><button>Click test</button><canvas width=900 height=450></canvas><script>
for(const t of ['pointerdown','pointermove','pointerup','pointercancel','click','contextmenu','keydown','keyup','input','focus','blur']) window.addEventListener(t,e=>{fetch('/events',{method:'POST',body:JSON.stringify({type:t,x:e.clientX,y:e.clientY,button:e.button,buttons:e.buttons,meta:e.metaKey,ctrl:e.ctrlKey,shift:e.shiftKey,trusted:e.isTrusted,focused:document.hasFocus(),value:document.querySelector('input').value})}); if(t==='pointerdown')e.target.setPointerCapture(e.pointerId);if(t==='contextmenu')e.preventDefault()},true);
</script>'''
class Server(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):self.send_response(200);self.end_headers();self.wfile.write(page.encode())
 def do_POST(self):records.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))));self.send_response(200);self.end_headers()
helper=subprocess.Popen([binary,'--server'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,bufsize=1)
def native(**args):
 helper.stdin.write(json.dumps(args)+'\n');helper.stdin.flush()
 if not select.select([helper.stdout],[],[],15)[0]:raise TimeoutError('Native response timeout')
 line=helper.stdout.readline()
 if not line:raise RuntimeError('Helper stopped: '+helper.stderr.read())
 r=json.loads(line)
 if 'error' in r:raise RuntimeError(r['error'])
 return r
with tempfile.TemporaryDirectory(prefix='lessagent-state-probe-',ignore_cleanup_errors=True) as d:
 server=ThreadingHTTPServer(('127.0.0.1',0),Server);threading.Thread(target=server.serve_forever,daemon=True).start()
 before=native(action='windows')['desktop'];pid=None;results=[]
 try:
  subprocess.run(['open','-gj','-n','-a','Google Chrome','--args','--user-data-dir='+d+'/chrome','--no-first-run','--no-default-browser-check','--window-size=1000,750','--app=http://127.0.0.1:'+str(server.server_port)],check=True)
  for _ in range(100):
   target=next((w for w in native(action='windows')['windows'] if w['title']=='Lessagent Native Probe'),None)
   if target:break
   time.sleep(.1)
  assert target;pid=target['pid']
  subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of first application process whose unix id is {before["frontmost_pid"]} to true'],capture_output=True)
  time.sleep(.5)
  for variant in [dict(probe_key_window=False),dict(probe_key_window=True,probe_skip_primer=True),dict(probe_key_window=True,probe_release=True),dict(probe_key_window=True)]:
   for args in [dict(action='click',x=220,y=97),dict(action='click',button='right',x=400,y=300),dict(action='move',x=450,y=330),dict(action='drag',x=350,y=350,to_x=650,to_y=500,duration=.25)]:
    records.clear()
    r=native(window_id=target['window_id'],pid=pid,show_pointer=False,**variant,**args)
    time.sleep(.15);result={'args':args,'variant':variant,'result':r,'events':list(records)}
    results.append(result);print(json.dumps(result),flush=True)
 finally:
  pathlib.Path('output/control-fix/state-probe-results.json').write_text(json.dumps(results,indent=2))
  if pid:subprocess.run(['kill','-TERM',str(pid)])
  helper.stdin.close();helper.wait(timeout=5)
  server.shutdown()
  time.sleep(.4)
