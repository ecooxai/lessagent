#!/usr/bin/env python3
"""Verify the deployed MCP on the visible local accuracy lab."""
import asyncio
import json
import math
import pathlib
import sys
import urllib.request
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

OUT=pathlib.Path(__file__).resolve().parent
WORKSPACE='ddd87f64723210031de1b2fd29533bc0'
BASE='http://127.0.0.1:4174'

def get_events():
    with urllib.request.urlopen(BASE+'/events',timeout=5) as response:return json.load(response)

def set_plan(points):
    request=urllib.request.Request(BASE+'/plan',data=json.dumps({'points':points}).encode(),headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(request,timeout=5) as response:return json.load(response)

async def main():
    headers={'Authorization':'Bearer '+(pathlib.Path.home()/'.local/share/lessagent/token').read_text().strip()}
    evidence={'passed':False,'actions':[],'accuracy':[]}
    async with streamablehttp_client('http://127.0.0.1:3210/mcp',headers=headers) as streams:
        async with ClientSession(streams[0],streams[1]) as session:
            await session.initialize()
            async def tool(name,args):
                response=await session.call_tool(name,{'workspace':WORKSPACE,**args})
                value=response.structuredContent['result']
                if response.isError:raise RuntimeError(value)
                return value
            if '--reuse' in sys.argv:
                existing=json.loads((OUT/'lab-target.json').read_text())
                target=await tool('computer',{'action':'screenshot','mode':'background','window_id':existing['window_id'],'pid':existing['pid']})
            else:
                target=await tool('browser_open',{'url':BASE+'/','width':1000,'height':750})
            (OUT/'lab-target.json').write_text(json.dumps(target,indent=2))
            last=target
            ready=next(e for e in reversed(get_events()) if e['type']=='ready')
            top=target['logical_height']-ready['innerHeight']
            async def action(label,**args):
                nonlocal last
                start=len(get_events())
                last=await tool('computer',{'mode':'background','window_id':target['window_id'],'pid':target['pid'],**args})
                assert last['delivery']=='chrome-devtools' and last['native_input_events_posted']==0,last
                assert last['before']['frontmost_pid']!=target['pid'] and last['after']['frontmost_pid']!=target['pid'],last
                await asyncio.sleep(.2)
                events=get_events()[start:]
                assert all(e.get('trusted',True) for e in events),events
                evidence['actions'].append({'label':label,'result':last,'events':events})
                print('PASS',label,flush=True)
                return events
            def pixel(x,y):
                return {'x':round(x*last['screen_width']/last['logical_width']),
                        'y':round((y+top)*last['screen_height']/last['logical_height']),
                        'screen_width':last['screen_width'],'screen_height':last['screen_height']}
            for x,y in [(250,180),(650,240),(450,520)]:
                set_plan([[x,y]]);await asyncio.sleep(.35)
                events=await action('click accuracy',action='click',**pixel(x,y))
                clicks=[e for e in events if e['type']=='click' and e.get('target')=='canvas']
                assert len(clicks)==1,clicks
                error=math.hypot(clicks[0]['x']-x,clicks[0]['y']-y)
                assert error<=1.01,error
                evidence['accuracy'].append({'action':'click','error_css_px':error})
            path=[[200,300],[400,400],[620,280]]
            set_plan(path);await asyncio.sleep(.35)
            sw,sh=last['screen_width'],last['screen_height']
            lw,lh=last['logical_width'],last['logical_height']
            events=await action('continuous drag accuracy',action='drag',path=[[x*sw/lw,(y+top)*sh/lh] for x,y in path],screen_width=sw,screen_height=sh,duration=.8)
            down=[e for e in events if e['type']=='pointerdown'];up=[e for e in events if e['type']=='pointerup']
            assert len(down)==len(up)==1 and down[0]['pointerType']=='pen' and up[0]['buttons']==0,(down,up)
            moves=[e for e in events if e['type']=='pointermove' and down[0]['seq']<e['seq']<up[0]['seq']]
            assert moves and all(e['buttons']==1 and e['pointerId']==down[0]['pointerId'] for e in moves),moves
            errors=[e['errorCSSPixels'] for e in moves]
            assert max(errors)<1.01,errors
            evidence['accuracy'].append({'action':'drag','samples':len(errors),'error_css_px':max(errors)})
            set_plan([[920,260]]);await asyncio.sleep(.35)
            events=await action('scroll position and panel isolation',action='scroll',distance=-4,**pixel(920,260))
            wheels=[e for e in events if e['type']=='wheel-detail']
            assert wheels and all(e['target']=='scrollbox' for e in wheels),wheels
            assert any(e['type']=='scrolled' and e['target']=='scrollbox' for e in events),events
            assert not any(e['type']=='scrolled' and e['target']=='scrollbox2' for e in events),events
            error=max(math.hypot(e['x']-920,e['y']-260) for e in wheels)
            assert error<=1.01,error
            evidence['accuracy'].append({'action':'scroll','error_css_px':error})
            evidence.update(passed=True,window_id=target['window_id'],pid=target['pid'],screenshot=last['path'])
    (OUT/'production-lab.json').write_text(json.dumps(evidence,indent=2))
    print('PASS deployed release MCP lab',flush=True)

if __name__=='__main__':asyncio.run(main())
