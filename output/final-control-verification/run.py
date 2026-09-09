#!/usr/bin/env python3
"""Run release verification independently of the service being restarted."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time
import traceback

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = pathlib.Path(__file__).resolve().parent
STATUS = OUT / 'status.json'
state = {'started': time.time(), 'pid': os.getpid(), 'complete': False, 'steps': []}

def save():
    temporary = STATUS.with_suffix('.tmp')
    temporary.write_text(json.dumps(state, indent=2))
    temporary.replace(STATUS)

def run(label, arguments, timeout=600, env=None):
    record = {'name': label, 'started': time.time(), 'arguments': arguments}
    state['steps'].append(record)
    save()
    with (OUT / (label + '.log')).open('w') as log:
        completed = subprocess.run(arguments, cwd=ROOT, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=subprocess.STDOUT, timeout=timeout, env=env)
    record.update(exit_code=completed.returncode, finished=time.time())
    save()
    if completed.returncode:
        raise RuntimeError(f'{label} exited with {completed.returncode}')

try:
    save()
    run('release-build', [str(pathlib.Path.home()/'.cargo/bin/cargo'), 'build', '--release', '--locked'])
    source = ROOT / 'target/release/lessagent'
    binary = OUT / 'lessagent-verified-release'
    shutil.copy2(source, binary)
    state['binary'] = str(binary)
    state['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
    state['sources'] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                        for p in list((ROOT/'src').glob('*.rs')) + list((ROOT/'native').glob('*.swift'))}
    save()
    env = dict(os.environ, LESSAGENT_CONTROL_REPORT_DIR=str(OUT.relative_to(ROOT)))
    uv = str(pathlib.Path.home()/'.local/bin/uv')
    run('background-integration', [uv, 'run', '--with', 'mcp>=1.20,<2', '--with', 'pillow',
        'tests/computer_background.py', str(binary)], env=env)
    run('unit', [str(pathlib.Path.home()/'.cargo/bin/cargo'), 'test', '--locked'])
    run('smoke', [sys.executable, 'tests/smoke.py', str(binary)])
    run('mcp', [uv, 'run', '--with', 'mcp>=1.20,<2', 'tests/mcp_client.py', str(binary)])
    run('startup', [sys.executable, 'tests/startup.py', str(binary)])
    state['passed'] = True
except BaseException as error:
    state['passed'] = False
    state['failure'] = str(error)
    state['traceback'] = traceback.format_exc()
finally:
    state.update(complete=True, finished=time.time())
    save()
