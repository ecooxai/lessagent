#!/usr/bin/env python3
"""Fast native-helper diagnostic; only a disposable background Chrome is targeted.
Usage: python3 tests/native_probe.py path/to/compiled-helper
This complements (not replaces) the full MCP/API integration regression.
"""
import collections
import json
import math
import pathlib
import subprocess
import sys
import tempfile
import threading
import time
from control_lab import LabServer

helper = pathlib.Path(sys.argv[1]).resolve()
managed = '--managed' in sys.argv
output = pathlib.Path(__file__).resolve().parents[1] / 'output/native-probe'
output.mkdir(parents=True, exist_ok=True)
lab = LabServer()
threading.Thread(target=lab.serve_forever, daemon=True).start()
chrome_pid = None
results = []
with tempfile.TemporaryDirectory(prefix='lessagent-native-probe-') as temporary:
    log = (output/'helper.log').open('w')
    process = subprocess.Popen([str(helper), '--server'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
    def call(**args):
        if managed: args["_browser_root"]=str(pathlib.Path(temporary)/"browsers")
        process.stdin.write(json.dumps(args)+'\n'); process.stdin.flush()
        line=process.stdout.readline()
        if not line: raise RuntimeError('Native helper exited: '+str(process.poll()))
        value=json.loads(line)
        if 'error' in value: raise RuntimeError(value['error'])
        return value
    def perform(label, **args):
        start=max((e['seq'] for e in lab.events),default=0)
        result=call(**target_args,**args)
        time.sleep(.35)
        events=sorted((e for e in lab.events if e['seq']>start),key=lambda e:e['seq'])
        summary={'label':label,'events':dict(collections.Counter(e['type'] for e in events)),
                 'down':[(e.get('x'),e.get('y'),e.get('buttons')) for e in events if e['type']=='pointerdown'],
                 'moves':dict(collections.Counter(e.get('buttons') for e in events if e['type']=='pointermove')),
                 'values':[(e.get('target'),e.get('value')) for e in events if e['type']=='input'],
                 'cursor_preserved':result['before']['cursor_x']==result['after']['cursor_x'] and result['before']['cursor_y']==result['after']['cursor_y'],
                 'foreground_preserved':result['before']['frontmost_pid']==result['after']['frontmost_pid']}
        print(json.dumps(summary,ensure_ascii=False),flush=True)
        results.append({'summary':summary,'result':result,'events':events})
        return events
    try:
        before=call(action='windows')['desktop']
        if managed:
            call(action='browser_open',url=f'http://127.0.0.1:{lab.server_port}/',width=1000,height=750)
        else:
            subprocess.run(['open','-gj','-n','-a','Google Chrome','--args',f'--user-data-dir={temporary}/chrome','--no-first-run','--no-default-browser-check','--disable-background-networking','--window-size=1000,750',f'--app=http://127.0.0.1:{lab.server_port}/'],check=True)
        for _ in range(100):
            target=next((w for w in call(action='windows')['windows'] if w['title']==f'Lessagent Background Drawing Test {lab.server_port}'),None)
            if target and any(e['type']=='ready' for e in lab.events):break
            time.sleep(.1)
        else:raise RuntimeError('No test browser window')
        chrome_pid=target['pid']
        # Do not change the human foreground application during diagnostics.
        time.sleep(.3)
        target_args={'window_id':target['window_id'],'pid':chrome_pid,'mode':'background'}
        shot=call(**target_args,action='screenshot',capture_path=str(output/'before.png'),show_pointer=False)
        ready=next(e for e in reversed(lab.events) if e['type']=='ready')
        top=shot['logical_height']-ready['innerHeight']
        def at(name,f=.5):
            r=ready['geometry'][name];return round(r['x']+r['width']*f),round(top+r['y']+r['height']*.5)
        x,y=at('range',.25);tx,ty=at('range',.9)
        perform('slider',action='drag',x=x,y=y,to_x=tx,to_y=ty,duration=.5)
        perform('canvas',action='drag',x=150,y=300,to_x=600,to_y=500,duration=.7)
        star=[[round(440+195*math.sin(i*4*math.pi/5)),round(435-195*math.cos(i*4*math.pi/5))] for i in range(6)]
        perform('star',action='drag',x=star[0][0],y=star[0][1],path=star,duration=3)
        perform('hover',action='move',x=410,y=400)
        x,y=at('name');perform('focus text',action='click',x=x,y=y)
        perform('unicode',action='type',text='cat 猫🐈X')
        perform('select last',action='key',key='shift+left')
        perform('replace last',action='type',text='!')
        perform('select all',action='key',key='cmd+a')
        perform('replace all',action='type',text='OK ✓')
        x,y=at('scrollbox');perform('scroll',action='scroll',x=x,y=y,delta=5)
        call(**target_args,action='screenshot',capture_path=str(output/'after.png'))
    finally:
        (output/'report.json').write_text(json.dumps(results,ensure_ascii=False,indent=2))
        process.stdin.close()
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:process.kill();process.wait()
        if chrome_pid:subprocess.run(['kill','-TERM',str(chrome_pid)],capture_output=True)
        lab.shutdown();lab.server_close();log.close()
