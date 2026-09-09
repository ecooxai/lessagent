#!/usr/bin/env python3
"""Real MCP SDK integration: uv run --with 'mcp>=1.20,<2' tests/mcp_client.py [binary]."""
import asyncio, base64, json, os, pathlib, signal, socket, struct, subprocess, sys, tempfile, time, zlib
import jsonschema
import urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.client.streamable_http import streamablehttp_client

BINARY = pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/lessagent').resolve()
PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='


class ProviderFixture(BaseHTTPRequestHandler):
    """Keep delegated-job tests deterministic and off real model accounts."""
    requests = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'models': [{'slug': 'gpt-5.4-mini', 'display_name': 'Fixture'}]}).encode())

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert self.headers['Authorization'] == 'Bearer fixture-token'
        assert 'MCP_SUMMARY_METADATA_ONLY' not in json.dumps(body), 'summary leaked into the delegated prompt'
        self.requests.append(body)
        output = [{'type': 'message', 'role': 'assistant', 'content': [
            {'type': 'output_text', 'text': 'MCP fixture complete <score>10</score><done><done>'}]}]
        event = {'type': 'response.completed', 'response': {'output': output, 'usage': {
            'input_tokens': 100, 'input_tokens_details': {'cached_tokens': 0}, 'output_tokens': 20}}}
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        self.wfile.write(('data: ' + json.dumps(event) + '\n\n').encode())


def payload(result):
    assert not result.isError, result
    structured=getattr(result,'structuredContent',None)
    if structured is not None:
        return structured['result']
    return json.loads(next(c.text for c in result.content if c.type=='text'))

def visible_text(result):
    return '\n'.join(c.text for c in result.content if c.type=='text')

async def exercise(session, root, label):
    init = await session.initialize()
    assert init.serverInfo.name == 'lessagent'
    assert all(word in init.instructions for word in ['Agents.md', 'AGENTS.md', 'read_file', 'first', 'summary', 'has_more'])
    await session.send_ping()
    definitions = {t.name:t for t in (await session.list_tools()).tools}
    assert {'bash','python','shell','computer','write_image','read_file'} <= definitions.keys()
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
        for invalid in [None, False, 7, [], {}, '', ' \t\r\n\u2003', 'x' * 1001, '雪' * 1001]:
            assert not jsonschema.Draft202012Validator(summary_schema).is_valid(invalid)
            rejected = await session.call_tool(tool.name, {'summary': invalid})
            assert rejected.isError and 'summary' in visible_text(rejected), (tool.name, rejected)
            assert 'summary' not in rejected.structuredContent, rejected
        rejected = await session.call_tool(tool.name, {})
        assert rejected.isError and 'summary' in visible_text(rejected), (tool.name, rejected)
        assert (await session.call_tool(tool.name, None)).isError
    assert 'distance' in definitions['computer'].inputSchema['properties']
    assert 'background-only' in definitions['computer'].description
    assert '4 KB excerpt' in definitions['read_file'].description and '8 KB' in definitions['read_file'].description
    called = set()
    async def invoke(name, **args):
        summary = args.pop('summary', f'Exercise {name} through {label} to verify the MCP contract.')
        result = await session.call_tool(name, dict(summary=summary, **args))
        called.add(name)
        assert result.structuredContent['summary'] == summary.strip(), (name, result)
        assert result.content[0].type == 'text'
        assert result.content[0].text == 'Call summary (client-provided intent): ' + summary.strip(), result
        return result

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
    assert missing.isError and 'Error:' in visible_text(missing)
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
        ('agent_run', {'prompt': 'Do not start this job without a summary.'}),
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
        assert 'bash-ok' in shown and 'Terminal:' in shown and not shown.lstrip().startswith('{'), shown
    code="from pathlib import Path\nprint(\"quotes ' \\\" $HOME `echo injected` \\nUnicode: 雪\")\nPath('python-result.txt').write_text('python-ok')"
    r=await run('python',code=code,wait_ms=1000)
    assert r['exit_code']==0 and '$HOME `echo injected`' in r['output'] and '雪' in r['output'], r
    assert (root/'python-result.txt').read_text()=='python-ok'
    long_text=('0123456789abcdef\n'*900)+'END-OF-FILE-MARKER'
    (root/'long.txt').write_text(long_text)
    first=await call('read_file',path='long.txt')
    first_data=payload(first); first_shown=visible_text(first)
    assert first_data['has_more'] and 1 <= first_data['returned_bytes'] <= 4000, first_data
    assert 'File excerpt: long.txt' in first_shown and 'END-OF-FILE-MARKER' not in first_shown
    assert len(first_shown) < len(long_text)//2, 'default MCP read_file must not dump the whole file'
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
    assert payload(result)['width']==1 and 'Saved image:' in visible_text(result)
    for result in [result,await call('read_file',path=f'{label}/pixel.png')]:
        image=next(c for c in result.content if c.type=='image')
        assert image.mimeType=='image/png' and base64.b64decode(image.data)==base64.b64decode(PIXEL)
    image_read=await call('read_file',path=f'{label}/pixel.png')
    assert 'Image file:' in visible_text(image_read)
    assert (root/label/'pixel.png').read_bytes()==base64.b64decode(PIXEL)
    for path,image in [('../escape.png',block),('bad.png',dict(block,data='not-base64')),('bad.jpg',block),('bad.png',dict(block,mimeType='image/jpeg'))]:
        assert (await call('write_image',path=path,image=image)).isError
    assert not (root/'bad.png').exists()
    # A real PNG larger than common 2 MiB HTTP defaults, with incompressible pixels.
    def chunk(kind, data):
        return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    pixels=b''.join(b'\0'+os.urandom(1024*3) for _ in range(800))
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',1024,800,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
    big=await call('write_image',path=f'{label}/large.png',image=dict(block,data=base64.b64encode(png).decode()))
    assert payload(big)['bytes']==len(png)
    assert base64.b64decode(next(c.data for c in big.content if c.type=='image'))==png
    assert (await call('python')).isError
    assert (await call('not_a_tool')).isError
    assert (await call('computer',action='click',x=1,y=1)).isError # Disabled by default.
    disabled = await call('browser_open', url='http://127.0.0.1:1/')
    assert disabled.isError and 'Enable computer control' in visible_text(disabled)
    inert = await run('bash', command='printf summary-is-metadata',
                     summary='Explain this call; do not execute $(touch summary-executed.txt).')
    assert inert['exit_code'] == 0 and 'summary-is-metadata' in inert['output']
    assert not (root / 'summary-executed.txt').exists()
    job = payload(await call('agent_run', prompt='Reply with the deterministic fixture greeting.',
                             summary='MCP_SUMMARY_METADATA_ONLY: verify delegation using the local model fixture.'))
    for _ in range(200):
        status = payload(await invoke('agent_status', job_id=job['job_id'], summary='Check whether the fixture job completed.'))
        if status['status'] != 'running':
            break
        await asyncio.sleep(.05)
    assert status['status'] == 'completed' and 'MCP fixture complete' in status['output'], status
    assert 'MCP_SUMMARY_METADATA_ONLY' not in status['prompt']
    assert ProviderFixture.requests, 'Delegated job never reached the local fixture'
    assert definitions.keys() <= called, definitions.keys() - called
    print(f'{label}: all {len(definitions)} tools, read-first guidance, summary validation/metadata, '
          'Bash/Python, PTY input/stop, file/image roundtrips, local agent run/status PASS')

async def main(root, port):
    data=root/'data'; work=root/'workspace'; work.mkdir()
    fixture = ThreadingHTTPServer(('127.0.0.1', 0), ProviderFixture)
    fixture_thread = threading.Thread(target=fixture.serve_forever, daemon=True)
    fixture_thread.start()
    auth = root / 'auth'; auth.mkdir()
    (auth / 'auth.json').write_text(json.dumps({'tokens': {'access_token': 'fixture-token', 'account_id': 'fixture-account'}}))
    env = {k: v for k, v in os.environ.items() if k not in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY')}
    env.update(CODEX_HOME=str(auth), LESSAGENT_CODEX_BASE_URL=f'http://127.0.0.1:{fixture.server_port}')
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
            fixture.shutdown(); fixture.server_close(); fixture_thread.join(timeout=5)

if __name__=='__main__':
    with tempfile.TemporaryDirectory(prefix='lessagent-mcp-test-') as tmp, socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]; sock.close()
        asyncio.run(main(pathlib.Path(tmp),port))
