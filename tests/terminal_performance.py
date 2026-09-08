#!/usr/bin/env python3
"""Real PTY regression: fast input/screen reads never persist app state."""
import concurrent.futures
import json, os, pathlib, signal, socket, statistics, subprocess, sys, tempfile, time, urllib.request
binary=pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/lessagent').resolve()
with tempfile.TemporaryDirectory() as directory:
    root=pathlib.Path(directory)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
    with (root/'server.log').open('w') as log:
        server=subprocess.Popen([str(binary),'--port',str(port),'--data-dir',str(root/'data'),'serve'],stdout=log,stderr=log)
        try:
            def request(path,body=None):
                req=urllib.request.Request(f'http://127.0.0.1:{port}{path}',data=json.dumps(body).encode() if body is not None else None,headers={'Content-Type':'application/json'})
                with urllib.request.urlopen(req,timeout=5) as response: return json.load(response)
            def act(name,body):return request('/api/action/'+name,body)
            for _ in range(100):
                try: request('/api/state'); break
                except OSError:time.sleep(.05)
            workspace=act('workspace_open',{'path':directory})['id']
            terminal=act('terminal_new',{'workspace':workspace})['id']
            time.sleep(.1)
            state_file=root/'data'/'state.json'; stamp=state_file.stat().st_mtime_ns
            old_logs=request('/api/state')['logs']
            timings=[]
            for i in range(20):
                marker=f'perf-{i:02d}'
                start=time.perf_counter()
                act('terminal_input',{'workspace':workspace,'terminal_id':terminal,'text':f"printf '{marker}\\n'\r"})
                deadline=time.monotonic()+3
                while True:
                    screen=act('terminal_screen',{'terminal_id':terminal})
                    text='\n'.join(''.join(c[0] for c in row) for row in screen['rows'])
                    if marker in text:break
                    assert time.monotonic()<deadline
                    time.sleep(.002)
                timings.append((time.perf_counter()-start)*1000)
            act('browse', {'path':directory})
            assert state_file.stat().st_mtime_ns==stamp, 'input, screen or browse request saved app state'
            summary=request('/api/state?summary=true')
            assert summary['logs']==old_logs, 'typing polluted tool logs'
            assert 'screen' not in summary['terminals'][0]
            assert 'screen' in request('/api/state')['terminals'][0]
            assert act('terminal_screen',{'terminal_id':terminal,'revision':screen['revision']})['unchanged']
            try:act('terminal_input',{'workspace':'wrong','terminal_id':terminal,'text':'x'})
            except urllib.error.HTTPError as error:assert error.code==400
            else:raise AssertionError('workspace ownership not enforced')
            # Background PTY output keeps advancing with no screen requests.
            act('terminal_input',{'workspace':workspace,'terminal_id':terminal,'text':"sleep .1; printf 'BACKGROUND_COMPLETE\\n'\r"})
            time.sleep(.3)
            screen=act('terminal_screen',{'terminal_id':terminal})
            assert any('BACKGROUND_COMPLETE'==''.join(c[0] for c in row).strip() for row in screen['rows'])
            # Compact runs preserve the rendered cells while eliminating per-cell JSON.
            act('terminal_input',{'workspace':workspace,'terminal_id':terminal,'text':"printf '\\033[31mRED\\033[0m normal 界\\n'\r"})
            time.sleep(.1)
            legacy=act('terminal_screen',{'terminal_id':terminal})
            compact=act('terminal_screen',{'terminal_id':terminal,'compact':True})
            def expand(row):
                return [(character,*run[1:]) for run in row for character in run[0]]
            assert [expand(r) for r in compact['rows']]==[expand(r) for r in legacy['rows']]
            ratio=len(json.dumps(compact))/len(json.dumps(legacy))
            assert ratio < .15, ratio
            # Idle requests sleep; two independent viewers both wake on real output.
            start=time.monotonic()
            idle=act('terminal_screen',{'terminal_id':terminal,'revision':compact['revision'],'wait_ms':250,'compact':True})
            assert idle['unchanged'] and time.monotonic()-start >= .20
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                pending=[pool.submit(act,'terminal_screen',{'terminal_id':terminal,'revision':compact['revision'],'wait_ms':2000,'compact':True}) for _ in range(2)]
                time.sleep(.1)
                assert all(not future.done() for future in pending)
                act('terminal_input',{'workspace':workspace,'terminal_id':terminal,'text':"printf 'WAKE_VIEWERS\\n'\r"})
                for future in pending:
                    assert 'rows' in future.result(timeout=1)
            lean=request('/api/state?summary=true&terminal=true')
            assert lean['partial'] and 'jobs' not in lean and 'logs' not in lean
            assert all('messages' not in w for w in lean['workspaces'])
            assert all('screen' not in t for t in lean['terminals'])
            print(f'PASS: compact screen equals styled/Unicode cells, payload {ratio:.1%} of legacy, idle wait and multiple viewer wakeup, history-free terminal state')
            print(f'PASS: no state saves/log entries, summary payload, revision checks, workspace ownership, background virtual screen; local input-to-screen median={statistics.median(timings):.1f}ms max={max(timings):.1f}ms')
        finally:
            server.send_signal(signal.SIGINT)
            try:server.wait(timeout=10)
            except subprocess.TimeoutExpired:server.kill();server.wait()
