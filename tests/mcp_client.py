#!/usr/bin/env python3
"""Real MCP SDK integration: uv run --with 'mcp>=1.20,<2' tests/mcp_client.py [binary]."""
import asyncio, base64, datetime, json, os, pathlib, signal, socket, struct, subprocess, sys, tempfile, time, zlib
import jsonschema
import urllib.request, urllib.error
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.client.streamable_http import streamablehttp_client

BINARY = pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/lessagent').resolve()
INSTRUCTION_URI = 'lessagent://server/instruction.md'
PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='


# Deterministic 7x5 files; no Pillow dependency is needed to execute this suite.
FORMAT_FIXTURES = {'jpeg': '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAAFAAcDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDzKiiivUPLP//Z', 'gif': 'R0lGODdhBwAFAIEAABQ8WgAAAAAAAAAAACwAAAAABwAFAAAIDAABCBxIsKDBgwcDAgA7', 'webp': 'UklGRjQAAABXRUJQVlA4ICgAAACQAQCdASoHAAUAAUAmJYgCdLoAA5gA/vjqf+j64RZGX+N8QXtSYAAA'}


def payload(result):
    assert not result.isError, result
    structured=getattr(result,'structuredContent',None)
    if structured is not None:
        return structured['result']
    return json.loads(next(c.text for c in result.content if c.type=='text'))

def visible_text(result):
    return '\n'.join(c.text for c in result.content if c.type=='text')

def check_image_metadata(result, width, height):
    data = payload(result)
    image = next(c for c in result.content if c.type == 'image')
    wire = image.model_dump(by_alias=True, exclude_none=True)
    meta = wire['_meta']['lessagent/image']
    assert (data['width'], data['height']) == (width, height)
    assert (meta['width'], meta['height']) == (width, height)
    assert (wire['width'], wire['height']) == (width, height)
    assert meta == data['image_metadata'] and meta['units'] == 'pixels'
    assert meta['mimeType'] == image.mimeType
    assert meta['bytes'] == len(base64.b64decode(image.data))
    assert 'fovea' not in wire

async def exercise(session, root, label):
    init = await session.initialize()
    assert init.serverInfo.name == 'lessagent'
    assert INSTRUCTION_URI in init.instructions
    for word in ['Agents.md', 'bash or python', 'instruction.md']:
        assert word in init.instructions, word
    assert 'read_file first' not in init.instructions
    assert init.capabilities.resources is not None
    assert not init.capabilities.resources.subscribe and not init.capabilities.resources.listChanged

    listed_resources = await session.list_resources()
    assert len(listed_resources.resources) == 1 and listed_resources.nextCursor is None
    resource = listed_resources.resources[0]
    assert resource.name == 'instruction.md' and str(resource.uri) == INSTRUCTION_URI
    assert resource.mimeType == 'text/markdown'
    assert not (await session.list_resource_templates()).resourceTemplates
    guide = (await session.read_resource(INSTRUCTION_URI)).contents[0]
    assert str(guide.uri) == INSTRUCTION_URI and guide.mimeType == 'text/markdown'
    for word in [
        'Agents.md', '1000 by 600', 'output/', 'final summary', 'CPU', 'GPU', 'RAM',
        'screen_width', 'current_task', 'progress', 'quality', 'managed Chrome profile',
        'browser_open` tool', 'bash', 'python', '1–500', 'get_files', 'send_files', 'normal Chrome profile',
    ]:
        assert word in guide.text, word
    system = guide.model_dump(by_alias=True)['_meta']['lessagent/system']
    assert system['os'] and system['process_architecture'] and system['observed_at_unix_ms'] > 0
    assert not system['computer_control_enabled'], 'Guide must be readable while computer control is disabled'
    if sys.platform == 'darwin':
        assert system['cpu']['logical_cores'] > 0 and system['ram']['total_bytes'] > 0
        assert isinstance(system['gpus'], list) and isinstance(system['displays'], list)

    await session.send_ping()
    definitions = {t.name:t for t in (await session.list_tools()).tools}
    expected = {
        'bash','python','shell','write_image','terminal_read','terminal_write','terminal_stop',
        'list_files','get_files','send_files','get_screenshot','virtual_pointer','virtual_keyboard','list_windows',
        'workspace_open','workspace_list','list_resources','read_resource','wait_n','browser_open','app_open',
    }
    assert expected <= definitions.keys(), expected - definitions.keys()
    for removed_name in ['read_file','write_file','computer','agent_run','agent_status']:
        assert removed_name not in definitions, removed_name

    string_meta = {
        'summary':500, 'agent':200, 'model':200, 'main_task':500,
        'current_task':300, 'current_timestamp':120,
    }
    for tool in definitions.values():
        jsonschema.Draft202012Validator.check_schema(tool.inputSchema)
        assert tool.description.startswith('Summary: '), (tool.name, tool.description)
        assert '\n\nPurpose: ' in tool.description and '\n\nHow: ' in tool.description
        assert 'Progress N/100' not in tool.description and 'Main task:' not in tool.description
        schema = tool.inputSchema
        for field,max_length in string_meta.items():
            assert field in schema['required'], (tool.name, field)
            field_schema = schema['properties'][field]
            assert field_schema['type'] == 'string' and field_schema['minLength'] == 1
            assert field_schema['pattern'] == r'\S' and field_schema['maxLength'] == max_length
        for field in ['progress','quality']:
            field_schema = schema['properties'][field]
            assert field in schema['required']
            assert field_schema['type'] == 'integer'
            assert field_schema['minimum'] == 0 and field_schema['maximum'] == 100
        assert list(schema['properties'])[0] == 'summary', tool.name
        assert list(schema['properties'])[-1] == 'current_timestamp', tool.name
        assert schema['required'][0] == 'summary' and schema['required'][-1] == 'current_timestamp'
        assert len(schema['required']) == len(set(schema['required']))
        summary_schema = schema['properties']['summary']
        assert len(summary_schema['description']) < 50
        for invalid in [None, False, 7, [], {}, '', ' \t\r\n\u2003', 'x' * 501, '雪' * 501]:
            assert not jsonschema.Draft202012Validator(summary_schema).is_valid(invalid)
            rejected = await session.call_tool(tool.name, {'summary': invalid})
            assert rejected.isError and 'summary' in visible_text(rejected), (tool.name, rejected)
        assert (await session.call_tool(tool.name, {})).isError
        assert (await session.call_tool(tool.name, None)).isError

    for name in ['list_resources','read_resource','workspace_list','get_files','get_screenshot','list_windows','wait_n']:
        annotations = definitions[name].annotations
        assert annotations.readOnlyHint and not annotations.destructiveHint
    for name in ['send_files','virtual_pointer','virtual_keyboard','app_open']:
        assert not definitions[name].annotations.readOnlyHint
    assert definitions['virtual_pointer'].inputSchema['properties']['action']['enum'] == ['move','click','drag','scroll']
    assert definitions['virtual_keyboard'].inputSchema['properties']['action']['enum'] == ['type','key']
    assert 'action' not in definitions['get_screenshot'].inputSchema['properties']
    assert 'workspace' not in definitions['wait_n'].inputSchema['properties']
    app_schema = definitions['app_open'].inputSchema
    assert app_schema['properties']['app']['type'] == 'string'
    assert app_schema['properties']['app']['minLength'] == 1 and 'app' in app_schema['required']
    assert app_schema['properties']['new_instance']['type'] == 'boolean'
    assert app_schema['properties']['new_instance']['default'] is True
    assert app_schema['additionalProperties'] is False and 'action' not in app_schema['properties']
    assert 'normal/default profile' in definitions['app_open'].description
    assert definitions['get_files'].inputSchema['properties']['paths']['maxItems'] == 64
    assert definitions['send_files'].inputSchema['properties']['files']['maxItems'] == 64

    called = set()
    async def invoke(name, **args):
        metadata = dict(
            summary=args.pop('summary', f'Ready; exercise {name}')[:500],
            agent=args.pop('agent', f'lessagent-{label}-integration'),
            model=args.pop('model', 'integration-test-model'),
            main_task=args.pop('main_task', 'Verify Lessagent MCP contract'),
            current_task=args.pop('current_task', f'Exercise {name} via {label}'),
            progress=args.pop('progress', 60),
            quality=args.pop('quality', 98),
            current_timestamp=args.pop(
                'current_timestamp', datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='milliseconds')
            ),
        )
        result = await session.call_tool(name, dict(**metadata, **args))
        called.add(name)
        structured = result.structuredContent
        for field in [*string_meta, 'progress', 'quality']:
            assert field not in structured, (name, field, structured)
        elapsed = structured['time_cost_ms']
        assert isinstance(elapsed, int) and elapsed >= 0
        assert result.content[0].type == 'text'
        prefix = 'Result: error · ' if result.isError else 'Result: ok · '
        assert result.content[0].text.splitlines()[0] == f'{prefix}{elapsed} ms'
        return result

    waited = await invoke('wait_n', seconds=0.02, summary='Ready; test wait')
    assert payload(waited)['waited_seconds'] == 0.02
    assert waited.structuredContent['time_cost_ms'] >= 10
    discovered = payload(await invoke('list_resources'))
    assert discovered['resources'][0]['uri'] == INSTRUCTION_URI
    via_tool = await invoke('read_resource', uri=INSTRUCTION_URI)
    assert payload(via_tool)['contents'][0]['uri'] == INSTRUCTION_URI
    assert (await invoke('read_resource', uri='file:///etc/passwd')).isError

    opened = await invoke('workspace_open', path=str(root), summary='Found folder; open workspace')
    assert 'Agents.md' in opened.structuredContent['instructions']
    workspace = payload(opened)['id']
    listed = await invoke('workspace_list', summary='Opened workspace; list it')
    assert payload(listed) and 'Agents.md' in listed.structuredContent['instructions']
    async def call(name, **args):
        return await invoke(name, workspace=workspace, **args)

    # Project guidance and file work now flow through Bash/Python, not MCP file tools.
    guidance_name = 'Agents.md' if label == 'http' else 'AGENTS.md'
    for candidate in ['Agents.md','AGENTS.md']:
        (root/candidate).unlink(missing_ok=True)
    guidance = '# Fixture guidance\nUse debug builds and isolated test ports.\nGUIDANCE-END\n'
    (root/guidance_name).write_text(guidance)
    read = payload(await call('bash', command=f"cat {guidance_name}", wait_ms=1000,
                              summary='Guide exists; read via bash'))
    while not read['exited']:
        read = payload(await call('terminal_read', terminal_id=read['terminal_id'], wait_ms=1000))
    assert 'GUIDANCE-END' in read['output']

    # Missing metadata must prevent executable side effects.
    for name,args in [
        ('bash', {'command':'touch blocked-command.txt'}),
        ('python', {'code':"from pathlib import Path; Path('blocked-python.txt').touch()"}),
    ]:
        result = await session.call_tool(name, dict(workspace=workspace, **args))
        assert result.isError and 'summary' in visible_text(result)
    assert not (root/'blocked-command.txt').exists() and not (root/'blocked-python.txt').exists()

    # Removed names stay rejected even with valid metadata.
    for name,args in [
        ('read_file', {'path':guidance_name}),
        ('write_file', {'path':'blocked.txt','text':'bad'}),
        ('computer', {'action':'click','x':1,'y':1}),
        ('agent_run', {'prompt':'unused'}),
        ('agent_status', {'job_id':'unused'}),
    ]:
        removed = await call(name, **args)
        assert removed.isError and f'Unknown tool: {name}' in visible_text(removed), removed
    assert not (root/'blocked.txt').exists()

    async def run(name, **args):
        r = payload(await call(name, **args))
        terminal_id = r['terminal_id']
        for _ in range(30):
            if r['exited']:
                return r
            r = payload(await call('terminal_read', terminal_id=terminal_id, wait_ms=1000))
        raise AssertionError('Program did not exit')

    for tool in ['shell','bash']:
        result = await run(tool, command="printf 'bash-ok\\n'; pwd", wait_ms=1000)
        assert result['exit_code'] == 0 and 'bash-ok' in result['output'] and str(root) in result['output']
    code = "from pathlib import Path\nPath('python-result.txt').write_text('python-ok')\nprint('雪')"
    result = await run('python', code=code, wait_ms=1000)
    assert result['exit_code'] == 0 and '雪' in result['output']
    assert (root/'python-result.txt').read_text() == 'python-ok'
    result = await run('bash', command="printf 'file-via-bash' > written.txt; cat written.txt")
    assert 'file-via-bash' in result['output'] and (root/'written.txt').read_text() == 'file-via-bash'
    assert payload(await call('list_files'))

    interactive = payload(await call('python', code="print('ready', flush=True); print('received:' + input())", wait_ms=100))
    assert not interactive['exited']
    terminal_id = interactive['terminal_id']
    payload(await call('terminal_write', terminal_id=terminal_id, text='hello\n'))
    for _ in range(20):
        interactive = payload(await call('terminal_read', terminal_id=terminal_id, wait_ms=1000))
        if interactive['exited']:
            break
    assert interactive['exit_code'] == 0 and 'received:hello' in interactive['output']
    sleeper = payload(await call('bash', command='sleep 60', wait_ms=0))
    payload(await call('terminal_stop', terminal_id=sleeper['terminal_id']))

    block = {'type':'image','mimeType':'image/png','data':PIXEL}
    uploaded = await call('write_image', path=f'{label}/pixel.png', image=block)
    check_image_metadata(uploaded, 1, 1)
    assert (root/label/'pixel.png').read_bytes() == base64.b64decode(PIXEL)
    for path,image in [
        ('../escape.png',block), ('bad.png',dict(block,data='not-base64')),
        ('bad.jpg',block), ('bad.png',dict(block,mimeType='image/jpeg')),
    ]:
        assert (await call('write_image', path=path, image=image)).isError
    for format,encoded in FORMAT_FIXTURES.items():
        image_result = await call('write_image', path=f'{label}/dimensions.{format}',
                                  image={'type':'image','mimeType':'image/'+format,'data':encoded})
        check_image_metadata(image_result, 7, 5)

    # Multi-file transfer preserves mixed media and arbitrary binary bytes.
    audio_bytes = b'ID3\x04\x00\x00lessagent-audio'
    video_bytes = b'\x00\x00\x00\x18ftypmp42lessagent-video'
    blend_bytes = b'BLENDER-v300lessagent-3d'
    binary_bytes = b'\x00\x01\xfe\xfflessagent-binary'
    batch = [
        {'path':f'{label}/batch/pixel.png','mimeType':'image/png','data':PIXEL},
        {'path':f'{label}/batch/sound.mp3','mimeType':'audio/mpeg','data':base64.b64encode(audio_bytes).decode()},
        {'path':f'{label}/batch/movie.mp4','mimeType':'video/mp4','data':base64.b64encode(video_bytes).decode()},
        {'path':f'{label}/batch/scene.blend','mimeType':'application/x-blender','data':base64.b64encode(blend_bytes).decode()},
        {'path':f'{label}/batch/raw.bin','data':base64.b64encode(binary_bytes).decode()},
    ]
    sent = await call('send_files', files=batch, summary='Ready; send mixed file batch')
    sent_data = payload(sent)
    assert sent_data['count'] == 5 and sent_data['total_bytes'] == sum(
        len(x) for x in [base64.b64decode(PIXEL), audio_bytes, video_bytes, blend_bytes, binary_bytes]
    )
    assert (root/label/'batch'/'sound.mp3').read_bytes() == audio_bytes
    assert (root/label/'batch'/'movie.mp4').read_bytes() == video_bytes
    assert (root/label/'batch'/'scene.blend').read_bytes() == blend_bytes
    assert (root/label/'batch'/'raw.bin').read_bytes() == binary_bytes
    assert sent_data['files'][-1]['mimeType'] == 'application/octet-stream'

    paths = [item['path'] for item in batch]
    received = await call('get_files', paths=paths, summary='Sent files; get mixed batch')
    received_data = payload(received)
    assert received_data['count'] == 5 and [f['kind'] for f in received_data['files']] == [
        'image','audio','video','file','file'
    ]
    blocks = [c.model_dump(by_alias=True, exclude_none=True) for c in received.content[1:]]
    assert [block['type'] for block in blocks] == ['image','audio','resource','resource','resource'], blocks
    assert base64.b64decode(blocks[0]['data']) == base64.b64decode(PIXEL)
    assert base64.b64decode(blocks[1]['data']) == audio_bytes
    assert base64.b64decode(blocks[2]['resource']['blob']) == video_bytes
    assert base64.b64decode(blocks[3]['resource']['blob']) == blend_bytes
    assert base64.b64decode(blocks[4]['resource']['blob']) == binary_bytes
    assert [block['_meta']['lessagent/file']['path'] for block in blocks] == paths
    assert blocks[2]['resource']['mimeType'] == 'video/mp4'
    assert blocks[3]['resource']['mimeType'] == 'application/x-blender'
    assert blocks[4]['resource']['mimeType'] == 'application/octet-stream'
    assert '_mcp_content' not in received_data

    for bad_files in [
        [{'path':'../escape.bin','data':base64.b64encode(b'x').decode()}],
        [{'path':f'{label}/batch/dup.bin','data':base64.b64encode(b'a').decode()},
         {'path':f'{label}/batch/dup.bin','data':base64.b64encode(b'b').decode()}],
        [{'path':f'{label}/batch/bad.bin','data':'not-base64'}],
    ]:
        assert (await call('send_files', files=bad_files)).isError
    assert (await call('get_files', paths=['../escape.bin'])).isError
    assert (await call('get_files', paths=[f'{label}/batch/missing.bin'])).isError

    # GUI tools remain advertised but reject while computer control is disabled.
    for name,args in [
        ('get_screenshot', {}), ('list_windows', {}),
        ('app_open', {'app':'Blender','new_instance':False,'summary':'雪' * 500}),
        ('virtual_pointer', {'action':'click','window_id':1,'pid':1,'x':1,'y':1}),
        ('virtual_keyboard', {'action':'key','window_id':1,'pid':1,'key':'x'}),
    ]:
        disabled = await call(name, **args)
        assert disabled.isError and 'Enable computer control' in visible_text(disabled), name

    # Metadata remains inert input and never reaches shell execution.
    inert = await run('bash', command='printf metadata-inert',
                      summary='Ready; verify metadata inert',
                      current_task='$(touch metadata-executed.txt)')
    assert inert['exit_code'] == 0 and 'metadata-inert' in inert['output']
    assert not (root/'metadata-executed.txt').exists()
    assert (await call('python')).isError
    assert (await call('not_a_tool')).isError
    print(f'{label}: MCP surface, metadata, resources, Bash/Python, PTY, mixed file transfer, images, removed tools PASS')

async def main(root, port):
    data=root/'data'; work=root/'workspace'; work.mkdir()
    env = {k: v for k, v in os.environ.items() if k not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY')}
    print(f'Debug MCP integration: binary={BINARY}, isolated port={port}')
    args=['--port',str(port),'--data-dir',str(data)]
    with (root/'server.log').open('w') as log:
        server=subprocess.Popen([str(BINARY),'serve',*args,'--passwd','mcp-test-password'],stdout=log,stderr=log,env=env)
        try:
            for _ in range(200):
                try:
                    urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/api/state',headers={'Authorization':'Bearer '+(data/'token').read_text().strip()}),timeout=1).close(); break
                except OSError: await asyncio.sleep(.05)
            else: raise AssertionError((root/'server.log').read_text())
            def rpc(body):
                req=urllib.request.Request(f'http://127.0.0.1:{port}/mcp',data=json.dumps(body).encode(),headers={'Content-Type':'application/json'})
                with urllib.request.urlopen(req) as response:
                    raw=response.read()
                    return response.status, json.loads(raw) if raw else None
            for path in ['/api/state','/api/admin/shutdown']:
                req=urllib.request.Request(f'http://127.0.0.1:{port}'+path,data=b'{}' if 'shutdown' in path else None,headers={'Content-Type':'application/json'})
                try: urllib.request.urlopen(req)
                except urllib.error.HTTPError as error: assert error.code==401
                else: raise AssertionError('Non-MCP authentication was removed')
            for authorization in [None, 'Bearer invalid']:
                headers={'Host':f'lessagent.test:{port}','Content-Type':'application/json'}
                if authorization: headers['Authorization']=authorization
                req=urllib.request.Request(f'http://127.0.0.1:{port}/mcp',data=json.dumps({'jsonrpc':'2.0','id':1,'method':'ping'}).encode(),headers=headers)
                with urllib.request.urlopen(req) as response: assert json.load(response)['result']=={}
            for version in ['2025-03-26','2025-06-18','2025-11-25','unknown']:
                _,r=rpc({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':version}})
                assert r['result']['protocolVersion']==(version if version!='unknown' else '2025-11-25')
            for body,code in [({'jsonrpc':'2.0','id':1,'method':'ping','params':[]},-32602),
                              ({'jsonrpc':'2.0','id':1,'method':'missing'},-32601),
                              ({'jsonrpc':'1.0','id':1,'method':'ping'},-32600)]:
                assert rpc(body)[1]['error']['code']==code
            for method, params, code in [
                ('resources/read', {}, -32602), ('resources/read', {'uri': None}, -32602),
                ('resources/read', {'uri': 'file:///etc/passwd'}, -32002),
                ('resources/read', {'uri': INSTRUCTION_URI + '?x=1'}, -32002),
                ('resources/list', {'cursor': 'bogus'}, -32602),
                ('resources/templates/list', {'cursor': 7}, -32602)]:
                assert rpc({'jsonrpc':'2.0','id':1,'method':method,'params':params})[1]['error']['code'] == code
            assert rpc({'jsonrpc':'2.0','method':'notifications/initialized'})==(202,None)
            assert rpc({'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':'workspace_list'}})[1]['result']['isError']
            for malformed in [None, [], 'not-an-object', 42, False]:
                response = rpc({'jsonrpc':'2.0','id':1,'method':'tools/call',
                                'params':{'name':'workspace_list','arguments':malformed}})[1]['result']
                assert response['isError'] and 'object' in response['structuredContent']['result']['error'], response
            assert not rpc({'jsonrpc':'2.0','id':1,'method':'tools/call',
                             'params':{'name':'workspace_list','arguments':{
                                 'summary':'Ready; list fixture workspaces',
                                 'agent':'raw-http-test',
                                 'model':'integration-test-model',
                                 'main_task':'Verify raw MCP HTTP tool calls',
                                 'current_task':'List workspaces',
                                 'progress':50,
                                 'quality':90,
                                 'current_timestamp':'2026-09-12T08:20:00-07:00',
                             }}})[1]['result']['isError']
            async with streamablehttp_client(f'http://127.0.0.1:{port}/mcp') as (read,write,_):
                async with ClientSession(read,write) as session: await exercise(session,work,'http')
            # Parse errors must stay on the protocol channel and not kill the bridge.
            lines='not json\n'+json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n'+json.dumps({'jsonrpc':'2.0','id':'after-error','method':'ping'})+'\n'
            bridge=subprocess.run([str(BINARY),'mcp',*args],input=lines,text=True,capture_output=True,timeout=10)
            replies=[json.loads(line) for line in bridge.stdout.splitlines()]
            assert bridge.returncode==0 and len(replies)==2, bridge.stderr
            assert replies[0]['error']['code']==-32700 and replies[1]['id']=='after-error'
            async with stdio_client(StdioServerParameters(command=str(BINARY),args=['mcp','--port',str(port),'--data-dir',str(root/'no-token-directory')])) as (read,write):
                async with ClientSession(read,write) as session: await exercise(session,work,'stdio')
        finally:
            server.send_signal(signal.SIGINT)
            try: server.wait(timeout=10)
            except subprocess.TimeoutExpired: server.kill(); server.wait()

if __name__=='__main__':
    with tempfile.TemporaryDirectory(prefix='lessagent-mcp-test-') as tmp, socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]; sock.close()
        asyncio.run(main(pathlib.Path(tmp),port))
