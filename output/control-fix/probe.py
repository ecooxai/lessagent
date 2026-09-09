import json, pathlib, subprocess, tempfile, threading, time, collections
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
ROOT=pathlib.Path.cwd(); binary=str(ROOT/'output/control-fix/probe'); records=[]
page='''<!doctype html><title>Lessagent Native Probe</title><style>body{margin:0}input{position:absolute;left:150px;top:45px;width:250px}button{position:absolute;left:500px;top:45px}canvas{position:absolute;top:150px;touch-action:none}</style><input><button>Click test</button><canvas width=900 height=450></canvas><script>
for(const t of ['pointerdown','pointermove','pointerup','pointercancel','click','contextmenu','keydown','keyup','input','focus','blur']) window.addEventListener(t,e=>{fetch('/events',{method:'POST',body:JSON.stringify({type:t,x:e.clientX,y:e.clientY,button:e.button,buttons:e.buttons,meta:e.metaKey,ctrl:e.ctrlKey,shift:e.shiftKey,trusted:e.isTrusted,focused:document.hasFocus(),value:document.querySelector('input').value})}); if(t==='pointerdown')e.target.setPointerCapture(e.pointerId);if(t==='contextmenu')e.preventDefault()},true);
</script>'''
class Server(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):self.send_response(200);self.end_headers();self.wfile.write(page.encode())
 def do_POST(self):records.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))));self.send_response(200);self.end_headers()
def native(**args):
 p=subprocess.run([binary],input=json.dumps(args),text=True,capture_output=True)
 if p.returncode:raise RuntimeError(p.stdout+p.stderr)
 return json.loads(p.stdout)
with tempfile.TemporaryDirectory(prefix='lessagent-native-probe-') as d:
 server=ThreadingHTTPServer(('127.0.0.1',0),Server);threading.Thread(target=server.serve_forever,daemon=True).start()
 before=native(action='windows')['desktop'];pid=None;results=[]
 try:
  subprocess.run(['open','-gj','-n','-a','Google Chrome','--args','--user-data-dir='+d+'/chrome','--no-first-run','--no-default-browser-check','--window-size=1000,750','--app=http://127.0.0.1:'+str(server.server_port)],check=True)
  for _ in range(100):
   target=next((w for w in native(action='windows')['windows'] if w['title']=='Lessagent Native Probe'),None)
   if target:break
   time.sleep(.1)
  assert target;pid=target['pid']
  for post,cg,stamp,subtype in [('public',False,True,3),('public',True,True,3),('sl',False,True,3),('sl',True,True,3),('sl',True,True,0)]:
   flags=0;tags=0
   subprocess.run(['osascript','-e','tell application "Finder" to activate'],capture_output=True);time.sleep(.6);records.clear()
   r=native(action='click',window_id=target['window_id'],pid=pid,x=220,y=97,flags=flags,subtype=subtype,tags=tags,show_pointer=False,post=post,cg=cg,stamp=stamp,focus=True,primer=True)
   time.sleep(.7);after=native(action='windows')['desktop']
   result={'post':post,'cg':cg,'stamp':stamp,'flags':flags,'subtype':subtype,'tags':tags,'result':r,'after_observation':after,'events':list(records)};results.append(result)
   print(json.dumps(result),flush=True)
 finally:
  pathlib.Path('output/control-fix/probe-results.json').write_text(json.dumps(results,indent=2))
  if pid:subprocess.run(['kill','-TERM',str(pid)])
  subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of first application process whose unix id is {before["frontmost_pid"]} to true'],capture_output=True)
  server.shutdown()
