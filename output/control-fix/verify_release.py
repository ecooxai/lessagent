#!/usr/bin/env python3
"""Run release verification without tying children to the live MCP backend PTY."""
import hashlib,json,os,pathlib,subprocess,time,traceback
ROOT=pathlib.Path('/Users/ecoo/project/agent/lessagent')
OUT=ROOT/'output/control-fix/release-verification';OUT.mkdir(parents=True,exist_ok=True)
os.chdir(ROOT)
env=os.environ.copy();env['NODE_PATH']=str(ROOT/'output/control-fix/test-deps/node_modules')
def fingerprint():
 files=[ROOT/'Cargo.toml',ROOT/'Cargo.lock',ROOT/'build.rs']+sorted((ROOT/'src').glob('*.rs'))+sorted((ROOT/'native').glob('*.swift'))+sorted((ROOT/'web').glob('*'))
 return {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files if p.is_file()}
report={'passed':False,'started':time.time(),'source_before':fingerprint(),'checks':[]}
def check(name,args,timeout=300):
 print('START',name,flush=True);started=time.time()
 with (OUT/(name+'.log')).open('w') as log:
  p=subprocess.run(args,stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT,env=env,timeout=timeout)
 result={'name':name,'exit_code':p.returncode,'seconds':round(time.time()-started,2)}
 report['checks'].append(result);(OUT/'report.json').write_text(json.dumps(report,indent=2))
 print('PASS' if p.returncode==0 else 'FAIL',name,flush=True)
 if p.returncode:raise RuntimeError(name+' failed; see '+str(OUT/(name+'.log')))
try:
 check('fmt',['cargo','fmt','--all','--','--check'])
 check('unit',['cargo','test','--locked'])
 check('clippy',['cargo','clippy','--all-targets','--locked','--','-D','warnings'])
 for test in sorted((ROOT/'tests').glob('*.cjs')):check('ui-'+test.stem,['node',str(test)])
 check('release-build',['cargo','build','--release','--locked'],600)
 binary=str(ROOT/'target/release/lessagent')
 report['binary_sha256']=hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()
 check('smoke',['python3','tests/smoke.py',binary],300)
 check('startup',['python3','tests/startup.py',binary],180)
 check('legacy-startup',['python3','tests/legacy_startup.py',binary],180)
 check('mcp-sdk',['uv','run','--with','mcp>=1.20,<2','tests/mcp_client.py',binary],300)
 env['LESSAGENT_CONTROL_REPORT_DIR']='output/control-fix/browser-release-report'
 check('background-controls',['uv','run','--with','mcp>=1.20,<2','--with','pillow','tests/computer_background.py',binary],420)
 report['source_after']=fingerprint()
 report['source_changed_during_run']=report['source_before']!=report['source_after']
 report['binary_unchanged']=report['binary_sha256']==hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()
 report['passed']=True
except BaseException as e:
 report['error']=str(e);report['traceback']=traceback.format_exc();print(report['traceback'],flush=True)
finally:
 report['finished']=time.time();(OUT/'report.json').write_text(json.dumps(report,indent=2))
 (OUT/'exit').write_text('0' if report['passed'] else '1')
