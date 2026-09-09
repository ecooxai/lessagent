#!/usr/bin/env python3
"""Draw a cat in Drawing Pro using the released computer tool over MCP stdio.
Only ordinary GUI tool actions paint pixels. No DOM/canvas drawing is injected.
The window is a disposable, isolated background Chrome profile and is left open.
"""
import asyncio,hashlib,json,math,pathlib,time,traceback
from mcp import ClientSession,StdioServerParameters
from mcp.client.stdio import stdio_client
ROOT=pathlib.Path('/Users/ecoo/project/agent/lessagent')
OUT=ROOT/'output/control-fix/cat-demo';OUT.mkdir(parents=True,exist_ok=True)
WORKSPACE='ddd87f64723210031de1b2fd29533bc0'
BINARY=ROOT/'target/release/lessagent'
report={'passed':False,'actions':[],'input':'computer tool over official MCP stdio; browser-local mouse/keyboard input'}
def unwrap(raw):
 assert not raw.isError,raw
 if raw.structuredContent:return raw.structuredContent['result']
 return json.loads(next(c.text for c in raw.content if c.type=='text'))
def curve(start,c1,c2,end,n=16):
 return [[(1-t)**3*start[0]+3*(1-t)**2*t*c1[0]+3*(1-t)*t*t*c2[0]+t**3*end[0],(1-t)**3*start[1]+3*(1-t)**2*t*c1[1]+3*(1-t)*t*t*c2[1]+t**3*end[1]] for t in [i/n for i in range(n+1)]]
def path(points):return [[round(330+x,2),round(226+y,2)] for x,y in points]
head=[[140,160],[134,143],[136,129],[143,116],[152,105],[149,85],[146,64],[145,43],[164,52],[185,67],[209,85],[226,80],[245,78],[263,80],[282,85],[305,66],[327,51],[345,42],[342,65],[340,88],[337,111],[346,124],[352,141],[353,158],[350,177],[343,196],[332,213],[317,225],[299,235],[279,242],[258,246],[236,246],[216,243],[196,237],[179,228],[164,216],[153,202],[146,187],[142,174],[140,160]]
body=curve((184,235),(152,280),(143,369),(164,399))+curve((164,399),(179,426),(295,424),(321,399))[1:]+curve((321,399),(346,374),(323,280),(294,237))[1:]
tail=curve((325,399),(421,424),(465,339),(415,288))+curve((415,288),(385,259),(357,288),(377,311))[1:]+curve((377,311),(425,337),(393,384),(329,371))[1:]
leftleg=curve((222,288),(211,314),(227,386),(210,409))
rightleg=curve((267,288),(278,321),(259,385),(278,412))
leftEye=curve((180,156),(190,141),(205,142),(214,156))
rightEye=curve((276,156),(287,141),(300,142),(310,156))
mouth=[[244,184],[244,198]]+curve((244,198),(237,213),(229,212),(224,200))[1:]+curve((224,200),(229,212),(237,213),(244,198))[1:]+curve((244,198),(250,212),(260,212),(265,200))[1:]
strokes=[('head',head,2.4),('body',body,1.8),('tail',tail,1.8),('left-foreleg',leftleg,.6),('right-foreleg',rightleg,.6),('left-eye',leftEye,.5),('right-eye',rightEye,.5),('mouth',mouth,.7),('whisker-left-top',[[176,179],[113,166]],.3),('whisker-left-bottom',[[174,197],[112,206]],.3),('whisker-right-top',[[314,179],[375,167]],.3),('whisker-right-bottom',[[316,197],[377,207]],.3),('left-paw-toes',[[183,401],[183,411],[195,414],[195,403]],.4),('right-paw-toes',[[290,404],[290,414],[304,411],[304,400]],.4)]
async def main():
 report['binary_sha256']=hashlib.sha256(BINARY.read_bytes()).hexdigest()
 verification=json.loads((ROOT/'output/control-fix/release-verification/report.json').read_text())
 assert verification['passed'] and report['binary_sha256']==verification['binary_sha256'],'Release binary is not the verified build'
 params=StdioServerParameters(command=str(BINARY),args=['mcp','--port','3210','--data-dir','/Users/ecoo/.local/share/lessagent'])
 async with stdio_client(params) as streams:
  async with ClientSession(*streams) as session:
   await session.initialize()
   opened=unwrap(await session.call_tool('browser_open',{'workspace':WORKSPACE,'url':'http://127.0.0.1:4173/','width':1050,'height':800,'capture_path':'output/control-fix/cat-demo/00-open.png'}))
   assert opened['delivery']=='chrome-devtools' and opened['isolated_profile'],opened
   target={'window_id':opened['window_id'],'pid':opened['pid']}
   report['target']=target;report['open']=opened
   (OUT/'report.json').write_text(json.dumps(report,indent=2))
   async def act(label,**args):
    raw=await session.call_tool('computer',dict(workspace=WORKSPACE,**target,**args,capture_path='output/control-fix/cat-demo/'+str(len(report['actions'])+1).zfill(2)+'-'+label+'.png'))
    r=unwrap(raw);report['actions'].append({'label':label,'arguments':args,'result':r})
    (OUT/'report.json').write_text(json.dumps(report,indent=2))
    assert r.get('automatic_screenshot') and not r.get('screenshot_error'),r
    assert r['logical_width']==1050 and r['logical_height']==800,('Window geometry changed; no further strokes will be sent',r)
    assert r['delivery']=='chrome-devtools' and r['native_input_events_posted']==0,r
    for field in ['before','after','desktop']:
     assert r[field]['frontmost_pid']!=target['pid'],('Demo window became foreground; stopping rather than continuing a background claim',field,r)
    assert any(c.type=='image' for c in raw.content),r
    print('PASS MCP',label,flush=True)
    return r
   await act('ready',action='move',x=850,y=740)
   await act('size-focus',action='click',x=704,y=150)
   await act('size-select',action='key',key='cmd+a')
   await act('size-five',action='type',text='5')
   await act('size-commit',action='key',key='tab')
   for label,points,duration in strokes:await act(label,action='drag',path=path(points),duration=duration)
   await act('pink',action='click',x=75,y=538)
   await act('left-inner-ear',action='drag',path=path([[161,69],[167,102],[190,95],[161,69]]),duration=.5)
   await act('right-inner-ear',action='drag',path=path([[328,69],[323,102],[303,95],[328,69]]),duration=.5)
   await act('nose',action='drag',path=path([[235,177],[253,177],[244,186],[235,177],[244,181],[249,178],[240,178],[244,182]]),duration=.5)
   final=await act('final',action='move',x=880,y=735,show_pointer=False)
   report['final_screenshot']=final['path'];report['passed']=True
   (OUT/'report.json').write_text(json.dumps(report,indent=2))
   print('CAT COMPLETE',target,final['path'],flush=True)
try:asyncio.run(main())
except BaseException as e:
 report['error']=str(e);report['traceback']=traceback.format_exc();print(report['traceback'],flush=True)
finally:
 (OUT/'report.json').write_text(json.dumps(report,indent=2));(OUT/'exit').write_text('0' if report['passed'] else '1')
