#!/usr/bin/env python3
"""Measure a release backend using a disposable copy of an existing data directory.
Usage: python3 tests/idle_performance.py target/release/lessagent DATA_DIR
macOS only: checks physical footprint with vmmap. Never starts copied jobs.
"""
import pathlib,tempfile,shutil,subprocess,socket,json,urllib.request,time,signal,re,sys
binary=str(pathlib.Path(sys.argv[1]).resolve()); src=pathlib.Path(sys.argv[2])
with tempfile.TemporaryDirectory(prefix='lessagent-idle-') as tmp:
 root=pathlib.Path(tmp)
 for name in ['state.json','token','password.hash']:
  if (src/name).exists(): shutil.copy2(src/name,root/name)
 shutil.copytree(src/'terminals',root/'terminals')
 if (src/'events').exists():shutil.copytree(src/'events',root/'events')
 with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
 token=(root/'token').read_text() if (root/'token').exists() else ''
 def req(path,body=None):
  r=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=json.dumps(body).encode() if body is not None else None,headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
  with urllib.request.urlopen(r,timeout=10) as response:return response.read()
 for phase in ['migration','clean-start']:
  with (root/'bench.log').open('a') as log:
   p=subprocess.Popen([binary,'serve','--port',str(port),'--data-dir',str(root)],stdout=log,stderr=log)
   try:
    for _ in range(200):
     try:req('/health');break
     except OSError:time.sleep(.1)
    def cpu():
     s=subprocess.check_output(['ps','-p',str(p.pid),'-o','time='],text=True).strip();m,sec=s.split(':');return int(m)*60+float(sec)
    start=time.monotonic();c=cpu();sizes=[]
    for _ in range(10):sizes.append(len(req('/api/state?summary=true')));time.sleep(1)
    usage=100*(cpu()-c)/(time.monotonic()-start)
    vm=subprocess.check_output(['vmmap','-summary',str(p.pid)],text=True,stderr=subprocess.DEVNULL)
    foot=re.search(r'Physical footprint:\s+([^\n]+)',vm)
    print(phase,'CPU',round(usage,2),'footprint',foot.group(1) if foot else '?','RSS KB',subprocess.check_output(['ps','-p',str(p.pid),'-o','rss='],text=True).strip(),'state bytes',sizes[-1],'saved bytes',(root/'state.json').stat().st_size,'terminals',len(list((root/'terminals').glob('*.json'))),flush=True)
    d=json.loads(req('/api/state?summary=true'))
    for event in [e for j in d['jobs'] for e in j['events'] if 'archived_event' in e][:1]:
     restored=json.loads(req('/api/action/event_read',{'id':event['archived_event']}))
     assert len(json.dumps(restored))>16384
    assert usage < 10, usage
    assert foot, 'vmmap did not report physical footprint'
    match=re.match(r'([0-9.]+)([KMG])', foot.group(1))
    assert match, foot.group(1)
    memory=float(match.group(1))*{'K':1024,'M':1024**2,'G':1024**3}[match.group(2)]
    assert memory < 100_000_000, memory
    assert len(list((root/'terminals').glob('*.json'))) <= 30
   finally:p.send_signal(signal.SIGINT);p.wait(timeout=15)
