#!/usr/bin/env python3
"""Deploy only the release binary that passed the complete verification run."""
import hashlib
import json
import pathlib
import subprocess
import time
import urllib.request

OUT=pathlib.Path(__file__).resolve().parent
ROOT=OUT.parents[1]
result={'started':time.time(),'passed':False}
try:
    status=json.loads((OUT/'status.json').read_text())
    assert status.get('complete') and status.get('passed'),'Verification has not passed'
    binary=pathlib.Path(status['binary'])
    expected=status['binary_sha256']
    assert hashlib.sha256(binary.read_bytes()).hexdigest()==expected,'Verified executable changed'
    changed=[name for name,digest in status['sources'].items() if hashlib.sha256((ROOT/name).read_bytes()).hexdigest()!=digest]
    assert not changed,('Sources changed after verification',changed)
    installed=ROOT/'target/release/lessagent'
    assert hashlib.sha256(installed.read_bytes()).hexdigest()==expected,'Release path differs from verified binary'
    with (OUT/'release-restart.log').open('w') as log:
        subprocess.run([str(installed),'restart','--port','3210','--data-dir',str(pathlib.Path.home()/'.local/share/lessagent')],
                       stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT,check=True,timeout=60)
    with urllib.request.urlopen('http://127.0.0.1:3210/health',timeout=5) as response:
        health=json.load(response)
    assert health.get('service')=='lessagent',health
    listeners=subprocess.check_output(['lsof','-nP','-t','-iTCP:3210','-sTCP:LISTEN'],text=True).split()
    assert len(set(listeners))==1,listeners
    pid=int(listeners[0])
    executable=subprocess.check_output(['ps','-p',str(pid),'-o','comm='],text=True).strip()
    assert executable==str(installed),(executable,installed)
    result.update(passed=True,pid=pid,executable=executable,binary_sha256=expected,health=health)
except BaseException as error:
    result['error']=str(error)
finally:
    result['finished']=time.time()
    (OUT/'deployment.json').write_text(json.dumps(result,indent=2))
    print(json.dumps(result,indent=2),flush=True)
