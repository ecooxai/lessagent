#!/usr/bin/env python3
"""Real MCP SDK integration: uv run --with 'mcp>=1.20,<2' tests/mcp_client.py [binary]."""
import asyncio, base64, json, os, pathlib, signal, socket, struct, subprocess, sys, tempfile, time, zlib
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
    assert all(word in init.instructions for word in ['Agents.md', 'AGENTS.md', 'read_file', 'first', 'summary', 'has_more', 'Progress 60/100'])
    assert INSTRUCTION_URI in init.instructions
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
    for word in ['Agents.md', '1000 by 600', 'output/', '3D', 'final summary', 'CPU', 'GPU', 'RAM', 'screen_width']:
        assert word in guide.text, word
    system = guide.model_dump(by_alias=True)['_meta']['lessagent/system']
    assert system['os'] and system['process_architecture'] and system['observed_at_unix_ms'] > 0
    assert not system['computer_control_enabled'], 'Guide must be readable while computer control is disabled'
    if sys.platform == 'darwin':
        assert 'probe_status' not in system, system
        assert system['cpu']['logical_cores'] > 0 and system['ram']['total_bytes'] > 0
        assert isinstance(system['gpus'], list) and isinstance(system['displays'], list)
    await session.send_ping()
    definitions = {t.name:t for t in (await session.list_tools()).tools}
    assert {'bash','python','shell','write_image','read_file','get_screenshot','virtual_pointer','virtual_keyboard','list_windows','app_open'} <= definitions.keys()
    for removed_name in ['computer', 'agent_run', 'agent_status']:
        assert removed_name not in definitions
    for tool in definitions.values():
        jsonschema.Draft202012Validator.check_schema(tool.inputSchema)
        assert tool.description.startswith('Summary: '), (tool.name, tool.description)
        assert '\n\nPurpose: ' in tool.description and '\n\nHow: ' in tool.description, (tool.name, tool.description)
        schema = tool.inputSchema
        assert 'summary' in schema['required'], tool.name
        assert len(schema['required']) == len(set(schema['required'])), tool.name
        summary_schema = schema['properties']['summary']
        assert summary_schema['type'] == 'string' and summary_schema['minLength'] == 1
        assert summary_schema['maxLength'] == 1000 and summary_schema['pattern'] == r'\S'
        assert 'Agents.md' in tool.description and 'AGENTS.md' in tool.description and 'Call summary:' in tool.description
        assert 'Progress 60/100' in tool.description and 'already been completed' in tool.description
        for invalid in [None, False, 7, [], {}, '', ' \t\r\n\u2003', 'x' * 1001, '雪' * 1001]:
            assert not jsonschema.Draft202012Validator(summary_schema).is_valid(invalid)
            rejected = await session.call_tool(tool.name, {'summary': invalid})
            assert rejected.isError and 'summary' in visible_text(rejected), (tool.name, rejected)
            assert 'summary' not in rejected.structuredContent, rejected
        rejected = await session.call_tool(tool.name, {})
        assert rejected.isError and 'summary' in visible_text(rejected), (tool.name, rejected)
        assert (await session.call_tool(tool.name, None)).isError
    for name in ['list_resources', 'read_resource', 'workspace_list', 'read_file', 'get_screenshot', 'list_windows']:
        annotations = definitions[name].annotations
        assert annotations.readOnlyHint and not annotations.destructiveHint and not annotations.openWorldHint
    for name in ['browser_open']:
        fields = definitions[name].inputSchema['properties']
        assert fields['width'] == system['browser_size_schema']['width']
        assert fields['height'] == system['browser_size_schema']['height']
        url_description = fields['url']['description']
        assert '?purpose=texttodescribepurposeofthiswindow_by_modelname' in url_description
        assert '&purpose=...' in url_description and 'URL-encode' in url_description
        assert 'existing persistent managed profile by default' in definitions[name].description
    if system.get('displays'):
        primary = next(display for display in system['displays'] if display['primary'])
        assert definitions['browser_open'].inputSchema['properties']['width']['maximum'] == primary['logical_width']
        assert definitions['browser_open'].inputSchema['properties']['height']['maximum'] == primary['logical_height']
    for name in ['get_screenshot', 'virtual_pointer', 'virtual_keyboard', 'list_windows', 'app_open']:
        assert definitions[name].inputSchema['additionalProperties'] is False
    assert 'action' not in definitions['get_screenshot'].inputSchema['properties']
    assert 'text' not in definitions['virtual_pointer'].inputSchema['properties']
    assert 'x' not in definitions['virtual_keyboard'].inputSchema['properties']
    assert definitions['virtual_pointer'].inputSchema['properties']['action']['enum'] == ['move','click','drag','scroll']
    assert definitions['virtual_keyboard'].inputSchema['properties']['action']['enum'] == ['type','key']
    for name in ['virtual_pointer', 'virtual_keyboard', 'app_open']:
        assert not definitions[name].annotations.readOnlyHint
    assert 'distance' in definitions['virtual_pointer'].inputSchema['properties']
    assert 'background' in definitions['virtual_pointer'].description
    assert '4 KB excerpt' in definitions['read_file'].description and '8 KB' in definitions['read_file'].description
    called = set()
    async def invoke(name, **args):
        summary = args.pop('summary', f'Exercise {name} through {label} to verify the MCP contract.')
        result = await session.call_tool(name, dict(summary=summary, **args))
        called.add(name)
        structured = result.structuredContent
        assert 'summary' not in structured, (name, structured)
        elapsed = structured['time_cost_ms']
        assert isinstance(elapsed, int) and elapsed >= 0, (name, elapsed)
        assert result.content[0].type == 'text'
        status = result.content[0].text
        prefix = 'Result: error · ' if result.isError else 'Result: ok · '
        assert status.startswith(prefix), (name, status)
        assert status.splitlines()[0] == f'{prefix}{elapsed} ms', (name, status, elapsed)
        if not result.isError:
            assert status == f'Result: ok · {elapsed} ms', (name, status)
        return result

    discovered = payload(await invoke('list_resources'))
    assert discovered['resources'][0]['uri'] == INSTRUCTION_URI
    via_tool = await invoke('read_resource', uri=INSTRUCTION_URI)
    bridge_guide = payload(via_tool)['contents'][0]
    assert bridge_guide['uri'] == INSTRUCTION_URI and bridge_guide['mimeType'] == 'text/markdown'
    assert bridge_guide['_meta']['lessagent/system']['observed_at_unix_ms'] >= system['observed_at_unix_ms']
    assert (await invoke('read_resource', uri='file:///etc/passwd')).isError
    assert (await invoke('read_resource')).isError

    opened = await invoke('workspace_open', path=str(root), summary='Open the fixture workspace before reading its guidance.')
    assert 'Agents.md' in opened.structuredContent['instructions']
    workspace = payload(opened)['id']
    listed = await invoke('workspace_list', summary='Find the opened fixture workspace and its read-first guidance.')
    assert payload(listed) and 'Agents.md' in listed.structuredContent['instructions']
    async def call(name, **args):
        return await invoke(name, workspace=workspace, **args)

    # Read-first bootstrap includes missing guidance, conventional casing, and pagination.
    guidance_name = 'Agents.md' if label == 'http' else 'AGENTS.md'
    for candidate in ['Agents.md', 'AGENTS.md']:
        (root / candidate).unlink(missing_ok=True)
    missing = await call('read_file', path=guidance_name, summary='Check for project guidance before any command or edit.')
    assert missing.isError and visible_text(missing).startswith('Result: error · ')
    guidance = '# Fixture guidance\n' + ('Use debug builds and isolated test ports.\n' * 220) + 'GUIDANCE-END'
    (root / guidance_name).write_text(guidance)
    offset = 0
    chunks = []
    while True:
        result = payload(await call('read_file', path=guidance_name, offset=offset,
                                    summary='Read all project guidance before working in the fixture.'))
        chunks.append(result['text'].removesuffix('\n[truncated]'))
        if not result['has_more']:
            break
        assert result['next_offset'] > offset
        offset = result['next_offset']
    assert ''.join(chunks) == guidance

    # Missing summary must not dispatch otherwise-valid destructive or executable arguments.
    for name, args in [
        ('write_file', {'path': 'blocked.txt', 'text': 'must not be written'}),
        ('bash', {'command': 'touch blocked-command.txt'}),
        ('python', {'code': "from pathlib import Path; Path('blocked-python.txt').touch()"}),
    ]:
        result = await session.call_tool(name, dict(workspace=workspace, **args))
        assert result.isError and 'summary' in visible_text(result), result
    assert all(not (root / path).exists() for path in ['blocked.txt', 'blocked-command.txt', 'blocked-python.txt'])

    for summary in ['x', '雪' * 1000, '🦀' * 1000, '  Read fixture workspaces.  ', 'Inspect workspaces.\nConfirm guidance is available.']:
        assert payload(await invoke('workspace_list', summary=summary))
    await call('write_file', path='written.txt', text='MCP write_file verified', summary='Save fixture text for a roundtrip check.')
    assert payload(await call('read_file', path='written.txt'))['text'] == 'MCP write_file verified'
    assert payload(await call('list_files'))

    async def run(name, **args):
        r=payload(await call(name,**args))
        for _ in range(30):
            if r['exited']: return r
            r=payload(await call('terminal_read',terminal_id=r['terminal_id'],wait_ms=1000))
        raise AssertionError('Program did not exit')
    for tool in ['shell','bash']:
        result=await call(tool,command="printf 'bash-ok\\n'; pwd",wait_ms=1000)
        r=payload(result)
        for _ in range(30):
            if r['exited']: break
            result=await call('terminal_read',terminal_id=r['terminal_id'],wait_ms=1000)
            r=payload(result)
        else: raise AssertionError('Program did not exit')
        assert r['exit_code']==0 and 'bash-ok' in r['output'] and str(root) in r['output'], r
        shown=visible_text(result)
        assert shown == result.content[0].text and shown.startswith('Result: ok · '), shown
    code="from pathlib import Path\nprint(\"quotes ' \\\" $HOME `echo injected` \\nUnicode: 雪\")\nPath('python-result.txt').write_text('python-ok')"
    r=await run('python',code=code,wait_ms=1000)
    assert r['exit_code']==0 and '$HOME `echo injected`' in r['output'] and '雪' in r['output'], r
    assert (root/'python-result.txt').read_text()=='python-ok'
    long_text=('0123456789abcdef\n'*900)+'END-OF-FILE-MARKER'
    (root/'long.txt').write_text(long_text)
    first=await call('read_file',path='long.txt')
    first_data=payload(first); first_shown=visible_text(first)
    assert first_data['has_more'] and 1 <= first_data['returned_bytes'] <= 4000, first_data
    assert first_shown.startswith('Result: ok · ') and 'END-OF-FILE-MARKER' not in first_shown
    assert len(first_shown) < 80, 'successful MCP status text should stay terse; data belongs in structuredContent'
    capped=payload(await call('read_file',path='long.txt',limit=100000))
    assert capped['has_more'] and capped['returned_bytes'] <= 8000, capped
    second=payload(await call('read_file',path='long.txt',offset=first_data['next_offset'],limit=1000))
    assert second['offset']==first_data['next_offset'] and second['returned_bytes'] <= 1000, second
    for name,args in [('bash',{'command':'echo failure >&2; exit 7'}),('python',{'code':"raise RuntimeError('expected-failure')"})]:
        r=await run(name,**args)
        assert r['exit_code'] != 0 and 'failure' in r['output'], r
    r=payload(await call('python',code="print('ready', flush=True); print('received:' + input())",wait_ms=100))
    assert not r['exited']
    payload(await call('terminal_write',terminal_id=r['terminal_id'],text='hello\n'))
    for _ in range(20):
        r=payload(await call('terminal_read',terminal_id=r['terminal_id'],wait_ms=1000))
        if r['exited']: break
    assert r['exit_code']==0 and 'received:hello' in r['output'],r
    r=payload(await call('bash',command='sleep 60',wait_ms=0))
    payload(await call('terminal_stop',terminal_id=r['terminal_id']))
    block={'type':'image','mimeType':'image/png','data':PIXEL}
    result=await call('write_image',path=f'{label}/pixel.png',image=block)
    assert payload(result)['width']==1 and visible_text(result).startswith('Result: ok · ')
    for result in [result,await call('read_file',path=f'{label}/pixel.png')]:
        image=next(c for c in result.content if c.type=='image')
        assert image.mimeType=='image/png' and base64.b64decode(image.data)==base64.b64decode(PIXEL)
        check_image_metadata(result, 1, 1)
    image_read=await call('read_file',path=f'{label}/pixel.png')
    assert visible_text(image_read).startswith('Result: ok · ')
    assert (root/label/'pixel.png').read_bytes()==base64.b64decode(PIXEL)
    for path,image in [('../escape.png',block),('bad.png',dict(block,data='not-base64')),('bad.jpg',block),('bad.png',dict(block,mimeType='image/jpeg'))]:
        assert (await call('write_image',path=path,image=image)).isError
    assert not (root/'bad.png').exists()
    for format, encoded in FORMAT_FIXTURES.items():
        path = f'{label}/dimensions.{format}'
        uploaded = await call('write_image', path=path, image={'type':'image','mimeType':'image/'+format,'data':encoded})
        reread = await call('read_file', path=path)
        for image_result in [uploaded, reread]:
            check_image_metadata(image_result, 7, 5)
            data = payload(image_result)
            assert data['format'] == format and data['image_metadata']['format'] == format
            assert base64.b64decode(next(c.data for c in image_result.content if c.type=='image')) == base64.b64decode(encoded)

    # A real PNG larger than common 2 MiB HTTP defaults, with incompressible pixels.
    def chunk(kind, data):
        return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    pixels=b''.join(b'\0'+os.urandom(1024*3) for _ in range(800))
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',1024,800,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
    big=await call('write_image',path=f'{label}/large.png',image=dict(block,data=base64.b64encode(png).decode()))
    assert payload(big)['bytes']==len(png)
    check_image_metadata(big, 1024, 800)
    assert base64.b64decode(next(c.data for c in big.content if c.type=='image'))==png
    assert (await call('python')).isError
    assert (await call('not_a_tool')).isError
    for name, args in [
        ('computer', {'action':'click','x':1,'y':1}),
        ('agent_run', {'prompt':'unused'}),
        ('agent_status', {'job_id':'unused'}),
    ]:
        removed = await call(name, **args)
        assert removed.isError and f'Unknown tool: {name}' in visible_text(removed), removed
    disabled = await call('browser_open', url='http://127.0.0.1:1/')
    assert disabled.isError and 'Enable computer control' in visible_text(disabled)
    for name, args in [
        ('get_screenshot', {}), ('list_windows', {}),
        ('virtual_pointer', {'action':'click','window_id':1,'pid':1,'x':1,'y':1}),
        ('virtual_keyboard', {'action':'key','window_id':1,'pid':1,'key':'x'}),
        ('app_open', {'app':'Blender'}),
    ]:
        disabled = await call(name, **args)
        assert disabled.isError and 'Enable computer control' in visible_text(disabled), name
    inert = await run('bash', command='printf summary-is-metadata',
                     summary='Explain this call; do not execute $(touch summary-executed.txt).')
    assert inert['exit_code'] == 0 and 'summary-is-metadata' in inert['output']
    assert not (root / 'summary-executed.txt').exists()
    assert definitions.keys() <= called, definitions.keys() - called
    print(f'{label}: all {len(definitions)} tools, read-first guidance, summary validation/metadata, '
          'resources/read+list, host info, image dimensions, Bash/Python, PTY input/stop, file/image roundtrips, removed agent job tools PASS')

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
                            'params':{'name':'workspace_list','arguments':{'summary':'List fixture workspaces.'}}})[1]['result']['isError']
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
