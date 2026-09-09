#!/usr/bin/env python3
"""Local, dependency-free native pointer accuracy lab.

Run: python3 tests/control_lab.py --port 4174
Open the printed address in a background Chrome window. The page observes real
input; /plan draws expected guides but NEVER generates input or changes focus.
GET /events exposes measured coordinates, button masks, trust and scroll offsets.
"""
import argparse
import json
import pathlib
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HTML = pathlib.Path(__file__).parent / 'fixtures/drawing.html'


class LabServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address=('127.0.0.1', 0)):
        super().__init__(address, LabHandler)
        self.events = []
        self.plan = {'id': 0, 'points': []}
        self.lock = threading.Lock()


class LabHandler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, body, content_type='application/json', status=200):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = self.path.split('?', 1)[0]
        if path == '/':
            self.reply(HTML.read_bytes(), 'text/html; charset=utf-8')
        elif path == '/events':
            with self.server.lock:
                snapshot = list(self.server.events)
            self.reply(snapshot)
        elif path == '/plan':
            with self.server.lock:
                plan = dict(self.server.plan)
            self.reply(plan)
        elif path == '/health':
            self.reply({'ok': True, 'input_injection': False})
        else:
            self.reply({'error': 'Not found'}, status=404)

    def do_POST(self):
        if self.path not in ('/event', '/plan'):
            return self.reply({'error': 'Not found'}, status=404)
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 < length <= 256_000:
                raise ValueError('Invalid body size')
            value = json.loads(self.rfile.read(length))
            if not isinstance(value, dict):
                raise ValueError('Expected JSON object')
            with self.server.lock:
                if self.path == '/event':
                    self.server.events.append(value)
                    if len(self.server.events) > 20_000:
                        del self.server.events[:1000]
                else:
                    points = value.get('points', [])
                    if not isinstance(points, list) or len(points) > 512:
                        raise ValueError('At most 512 points')
                    for point in points:
                        if not isinstance(point, list) or len(point) != 2:
                            raise ValueError('Points must be [x,y]')
                        if any(type(v) not in (int, float) or not -10000 <= v <= 10000 for v in point):
                            raise ValueError('Invalid coordinate')
                    self.server.plan = {**value, 'id': self.server.plan['id'] + 1}
                    value = dict(self.server.plan)
            self.reply({'ok': True, **({'plan': value} if self.path == '/plan' else {})})
        except (ValueError, TypeError, json.JSONDecodeError) as error:
            self.reply({'error': str(error)}, status=400)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4174)
    args = parser.parse_args()
    server = LabServer(('127.0.0.1', args.port))
    print(f'Pointer Lab: http://127.0.0.1:{server.server_port}/', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
