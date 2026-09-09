#!/usr/bin/env python3
"""Draw the verification cat using only the running MCP computer tool.
No JavaScript, DOM events, canvas APIs, clipboard writes or global input.
"""
import asyncio
import json
import pathlib
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

OUT = pathlib.Path(__file__).resolve().parent
WORKSPACE = 'ddd87f64723210031de1b2fd29533bc0'

# Hand-planned curves are sampled into mouse paths, not raster/vector artwork.
def curve(*commands):
    points=[]
    for command in commands:
        op,*v=command
        if op in ('M','L'):
            points.append([v[0],v[1]])
        elif op=='Q':
            x,y=points[-1]
            for i in range(1,13):
                t=i/12;u=1-t
                points.append([u*u*x+2*u*t*v[0]+t*t*v[2],u*u*y+2*u*t*v[1]+t*t*v[3]])
        elif op=='C':
            x,y=points[-1]
            for i in range(1,17):
                t=i/16;u=1-t
                points.append([u**3*x+3*u*u*t*v[0]+3*u*t*t*v[2]+t**3*v[4],u**3*y+3*u*u*t*v[1]+3*u*t*t*v[3]+t**3*v[5]])
    return points

STROKES = [
 ('tail',curve(('M',647,619),('C',719,651,756,559,728,494),('C',718,473,691,476,690,496),('C',689,516,705,525,709,547),('C',719,584,692,600,660,592))),
 ('body',curve(('M',468,466),('C',454,503,456,552,450,589),('C',439,619,451,638,480,638),('L',618,638),('C',650,638,666,614,651,584),('C',646,529,624,496,616,471))),
 ('head',curve(('M',430,359),('L',423,270),('Q',468,278,489,316),('C',524,300,567,302,599,316),('L',650,270),('L',654,359),('C',686,410,661,462,607,483),('C',558,504,507,497,469,478),('C',416,460,405,410,430,359))),
 ('left ear',curve(('M',439,341),('L',438,293),('L',475,322))),
 ('right ear',curve(('M',617,322),('L',637,294),('L',639,343))),
 ('left eye',curve(('M',462,399),('Q',482,378,503,400))),
 ('right eye',curve(('M',582,400),('Q',602,378,622,402))),
 ('nose',curve(('M',535,419),('L',551,419),('L',543,428),('L',535,419))),
 ('left smile',curve(('M',543,428),('L',543,438),('C',536,453,522,451,517,440))),
 ('right smile',curve(('M',543,438),('C',551,453,565,451,569,440))),
 ('left whisker 1',curve(('M',468,424),('Q',433,414,409,413))),
 ('left whisker 2',curve(('M',470,437),('L',405,439))),
 ('left whisker 3',curve(('M',476,449),('Q',441,457,414,467))),
 ('right whisker 1',curve(('M',616,425),('Q',646,414,674,413))),
 ('right whisker 2',curve(('M',614,438),('L',683,440))),
 ('right whisker 3',curve(('M',608,451),('Q',645,460,672,471))),
 ('chest fur',curve(('M',490,489),('L',502,505),('L',513,499),('L',526,515),('L',541,505),('L',552,519),('L',568,504),('L',582,512),('L',600,490))),
 ('left leg',curve(('M',503,535),('Q',510,567,502,606),('C',484,609,484,635,508,638))),
 ('right leg',curve(('M',584,534),('Q',579,574,585,607),('C',604,611,602,637,578,638))),
 ('left toe 1',curve(('M',500,620),('L',500,636))),
 ('left toe 2',curve(('M',512,622),('L',512,638))),
 ('right toe 1',curve(('M',577,622),('L',577,638))),
 ('right toe 2',curve(('M',588,620),('L',588,635))),
 ('forehead stripe 1',curve(('M',504,330),('Q',515,344,519,359))),
 ('forehead stripe 2',curve(('M',546,320),('L',546,349))),
 ('forehead stripe 3',curve(('M',586,332),('Q',576,346,572,359))),
]

async def main():
    target=json.loads((OUT/'paint-target.json').read_text())
    headers={'Authorization':'Bearer '+(pathlib.Path.home()/'.local/share/lessagent/token').read_text().strip()}
    results=[]
    last=target
    async with streamablehttp_client('http://127.0.0.1:3210/mcp',headers=headers) as streams:
        async with ClientSession(streams[0],streams[1]) as session:
            await session.initialize()
            async def action(label,**args):
                nonlocal last
                payload={'workspace':WORKSPACE,'mode':'background','window_id':target['window_id'],'pid':target['pid'],**args}
                response=await session.call_tool('computer',payload)
                result=response.structuredContent['result']
                if response.isError:raise RuntimeError(result)
                if args['action']!='screenshot':
                    assert result.get('delivery')=='chrome-devtools',result
                    assert result.get('native_input_events_posted')==0,result
                    assert result.get('automatic_screenshot'),result
                assert result['logical_width']==1000 and result['logical_height']==750,('Window resized',result)
                result['label']=label;results.append(result);last=result
                (OUT/'cat-actions.json').write_text(json.dumps(results,indent=2))
                print('PASS',label,flush=True)
            def position(x,y):
                return dict(x=round(x*last['screen_width']/1000),y=round(y*last['screen_height']/750),screen_width=last['screen_width'],screen_height=last['screen_height'])
            await action('initial paint observation',action='screenshot')
            await action('stroke input',action='click',**position(710,151))
            await action('select stroke value',action='key',key='cmd+a')
            await action('set stroke to 6',action='type',text='6')
            await action('commit stroke value',action='key',key='tab')
            for label,path in STROKES:
                sw,sh=last['screen_width'],last['screen_height']
                scaled=[[x*sw/1000,y*sh/750] for x,y in path]
                await action(label,action='drag',path=scaled,screen_width=sw,screen_height=sh,duration=min(1.8,max(.15,len(path)*.014)),
                    capture_path='output/final-control-verification/cat-paint.png')
            (OUT/'cat-status.json').write_text(json.dumps({'passed':True,'strokes':len(STROKES),'actions':len(results),
                'window_id':target['window_id'],'pid':target['pid'],'native_input_events_posted':0,'screenshot':last['path']},indent=2))

if __name__=='__main__':
    asyncio.run(main())
