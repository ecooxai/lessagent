#!/usr/bin/env python3
"""Verify real pointer panel pixels/timers without changing the system pointer.
Usage: uv run --with pillow tests/pointer_idle.py compiled-native-helper
"""
import json
import pathlib
import subprocess
import sys
import tempfile
import time
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT/'output/pointer-idle-probe'
OUT.mkdir(parents=True, exist_ok=True)
helper = pathlib.Path(sys.argv[1]).resolve()
results = []
pid = None
with tempfile.TemporaryDirectory(prefix='lessagent-pointer-idle-') as temporary:
    log = (OUT/'helper.log').open('w')
    process = subprocess.Popen([str(helper),'--server'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True)
    def call(**args):
        args['_browser_root'] = temporary
        process.stdin.write(json.dumps(args)+'\n');process.stdin.flush()
        line = process.stdout.readline()
        if not line: raise RuntimeError('Pointer helper exited')
        value = json.loads(line)
        if 'error' in value: raise RuntimeError(value['error'])
        return value
    def capture(panel, label):
        path = OUT/(label+'.png')
        subprocess.run(['/usr/sbin/screencapture','-x','-o','-l',str(panel),str(path)],check=True,capture_output=True)
        image=Image.open(path).convert('RGBA')
        pixels=list(image.get_flattened_data() if hasattr(image,'get_flattened_data') else image.getdata())
        result={'size':image.size,'blue_pixels':sum(a>100 and r<180 and g>130 and b>190 and b-r>40 for r,g,b,a in pixels),
                'nontransparent_pixels':sum(a>0 for r,g,b,a in pixels),'alpha_range':image.getextrema()[3]}
        print(label,result,flush=True)
        return result
    try:
        before=call(action='windows')['desktop']
        target=call(action='browser_open',url='http://127.0.0.1:4174/',width=1000,height=750)
        pid=target['pid']
        args={'window_id':target['window_id'],'pid':pid,'mode':'background'}
        time.sleep(.4)
        value=call(**args,action='move',x=400,y=400)
        epoch=time.monotonic()-value['pointer']['idle_seconds']
        panel=value['pointer']['panel_window_id']
        pixels=capture(panel,'active')
        assert pixels['blue_pixels']>30, pixels
        for seconds,phase in [(9.0,'active'),(10.4,'transparent'),(29.0,'transparent'),(30.4,'hidden')]:
            time.sleep(max(0,epoch+seconds-time.monotonic()))
            result=call(action='windows')
            pointer=result['pointer']
            assert pointer['phase']==phase,pointer
            assert pointer['panel_visible']==(phase!='hidden'),pointer
            assert result['desktop']['frontmost_pid']!=pid,result['desktop']
            sample={'requested_seconds':seconds,'pointer':pointer}
            if phase=='transparent' and seconds<11:
                pixels=capture(panel,'transparent')
                sample['pixels']=pixels
                assert pixels['blue_pixels']==0 and pixels['nontransparent_pixels']>0,pixels
            results.append(sample)
            print('PASS',seconds,phase,flush=True)
        value=call(**args,action='move',x=450,y=400)
        assert value['pointer']['phase']=='active' and value['pointer']['panel_visible'],value
        assert capture(panel,'reactivated')['blue_pixels']>30
        print('PASS reactivation',flush=True)
    finally:
        (OUT/'report.json').write_text(json.dumps(results,indent=2))
        process.stdin.close()
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:process.kill();process.wait()
        if pid:subprocess.run(['kill','-TERM',str(pid)],capture_output=True)
        log.close()
