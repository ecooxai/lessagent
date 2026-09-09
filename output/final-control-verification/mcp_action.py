#!/usr/bin/env python3
"""Exercise the running service over MCP, saving metadata separately from images."""
import argparse
import asyncio
import json
import pathlib
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('tool')
    parser.add_argument('arguments', help='JSON object')
    parser.add_argument('--output', required=True)
    parser.add_argument('--port', type=int, default=3210)
    args = parser.parse_args()
    token_file = pathlib.Path.home()/'.local/share/lessagent/token'
    headers = {'Authorization': 'Bearer '+token_file.read_text().strip()} if token_file.exists() else {}
    async with streamablehttp_client(f'http://127.0.0.1:{args.port}/mcp', headers=headers) as streams:
        async with ClientSession(streams[0], streams[1]) as session:
            await session.initialize()
            value = await session.call_tool(args.tool, json.loads(args.arguments))
            structured = getattr(value, 'structuredContent', None)
            if isinstance(structured, dict) and 'result' in structured:
                result = structured['result']
            else:
                result = {'text': [c.text for c in value.content if c.type == 'text']}
            output = pathlib.Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(result, indent=2))
            if value.isError:
                raise RuntimeError(result)
            summary = {k: result[k] for k in ['ok','action','window_id','pid','delivery','isolated_profile',
                'screen_width','screen_height','logical_width','logical_height','path','pointer'] if k in result}
            print(json.dumps(summary or result, indent=2))

if __name__ == '__main__':
    asyncio.run(main())
