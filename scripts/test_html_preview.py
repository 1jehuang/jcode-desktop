#!/usr/bin/env python3
"""Real WebKit integration tests. All browser windows use a private Xvfb.
Run: python3 scripts/test_html_preview.py
"""
import base64
import importlib.util
import io
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import sys
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from PIL import Image, ImageChops

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
RUNTIME = ROOT / 'crates/jcode-desktop-ui/src/html_preview_runtime.py'
spec = importlib.util.spec_from_file_location('html_runtime', RUNTIME)
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class Browser:
    def __init__(self):
        self.process = subprocess.Popen(['/usr/bin/python3', '-I', '-u', str(RUNTIME)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8',
                 'TMPDIR': os.environ.get('JCODE_SCRATCH_DIR', str(ROOT / 'target'))},
            text=True, start_new_session=True)
        self.events = queue.Queue()
        def read():
            for line in self.process.stdout:
                self.events.put(json.loads(line))
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        self.wait(lambda x: x['type'] == 'ready')

    def send(self, **value):
        self.process.stdin.write(json.dumps(value) + '\n')
        self.process.stdin.flush()

    def wait(self, predicate, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                event = self.events.get(timeout=max(.01, deadline-time.monotonic()))
            except queue.Empty:
                break
            if event['type'] == 'error':
                raise AssertionError(event['message'])
            if predicate(event):
                return event
        raise AssertionError('Browser did not produce expected output')

    def frame(self, predicate=lambda image: True):
        def matches(event):
            if event['type'] != 'frame':
                return False
            self.image = Image.open(io.BytesIO(base64.b64decode(event['png']))).convert('RGB')
            return predicate(self.image)
        self.wait(matches)
        return self.image

    def click(self, x, y):
        self.send(type='down', x=x, y=y)
        self.send(type='up', x=x, y=y)

    def close(self):
        try:
            self.send(type='quit')
            self.process.wait(timeout=5)
        finally:
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.process.wait(timeout=5)
            self.process.stdin.close()
            self.process.stdout.close()
            self.process.stderr.close()
            self.reader.join(timeout=2)


class EnvelopeTests(unittest.TestCase):
    def test_escape_prevents_srcdoc_breakout(self):
        value = runtime.envelope('\"><script>parent.document.body.remove()</script>')
        self.assertIn('&quot;&gt;&lt;script&gt;', value)
        self.assertEqual(value.count('<iframe '), 1)
        self.assertNotIn('allow-same-origin', value)
        self.assertNotIn('allow-top-navigation', value)

    def test_size_limit_is_utf8_bytes(self):
        with self.assertRaises(ValueError):
            runtime.envelope('界' * (runtime.MAX_SOURCE // 3 + 1))

    def test_policy_blocks_connections_and_files(self):
        for rule in ("default-src 'none'", "connect-src 'none'", "worker-src 'none'", "form-action 'none'", "base-uri 'none'"):
            self.assertIn(rule, runtime.CSP)
        self.assertNotIn('file:', runtime.CSP)
        self.assertNotIn('https:', runtime.CSP)


class BrowserTests(unittest.TestCase):
    def setUp(self):
        self.browser = Browser()
        self.addCleanup(self.browser.close)

    def test_renders_real_fonts_click_keyboard_resize_and_scroll(self):
        self.browser.send(type='load', html='''<style>body{margin:0;background:white}button{width:180px;height:60px}input{display:block;width:200px;height:40px}#bottom{position:absolute;top:1100px;width:100%;height:400px;background:blue}</style>
<button onclick="document.body.style.background='rgb(0,255,0)'">Click</button><input oninput="document.body.style.background='rgb(255,0,0)'">
<p style="font:30px Inter">Real sans serif</p><p style="font:30px Liberation Serif">Real serif</p><div id="bottom"></div>''')
        initial = self.browser.frame()
        self.assertEqual(initial.size, (1600, 840))
        self.assertNotEqual(initial.crop((0,220,600,480)).getextrema(), ((255,255),) * 3)
        self.browser.click(50, 25)
        self.browser.frame(lambda i: i.getpixel((1000,500)) == (0,255,0))
        self.browser.click(50, 80)
        self.browser.send(type='key', key='a', shift=False)
        self.browser.frame(lambda i: i.getpixel((1000,500)) == (255,0,0))
        self.browser.send(type='resize', width=600, height=500)
        self.browser.frame(lambda i: i.size == (1200,1000))
        self.browser.send(type='scroll', x=400, y=300, dx=0, dy=40)
        self.browser.frame(lambda i: i.getpixel((600,900)) == (0,0,255))

    def test_document_cannot_escape_or_make_network_requests(self):
        requests = []
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                self.send_response(200); self.end_headers(); self.wfile.write(b'forbidden')
            def log_message(self, *args):
                pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        url = 'http://127.0.0.1:' + str(server.server_port)
        source = '''<body style="margin:0;background:red"><img src="URL/image"><link rel="stylesheet" href="URL/css">
<script>
(async()=>{
let blocked=0;
try { parent.document.body.innerHTML='escaped'; } catch(e) { blocked++; }
try { localStorage.setItem('secret','x'); } catch(e) { blocked++; }
try { await fetch('URL/fetch'); } catch(e) { blocked++; }
try { await fetch('file:///etc/passwd'); } catch(e) { blocked++; }
await new Promise(resolve=>{try {let w=new Worker('URL/worker');w.onerror=()=>{blocked++;resolve()};setTimeout(resolve,1000)} catch(e) {blocked++;resolve()}});
if(blocked===5)document.body.style.background='rgb(0,255,0)';
})();
</script>'''.replace('URL', url)
        self.browser.send(type='load', html=source)
        self.browser.frame(lambda i: i.getpixel((600,400)) == (0,255,0))
        time.sleep(.3)
        self.assertEqual(requests, [])

    def test_font_sampler_runs_and_slider_changes_rendering(self):
        self.browser.send(type='load', html=(ROOT / 'assets/previews/font-pairings.html').read_text())
        before = self.browser.frame()
        # Slider location is stable at the initial 800 CSS px viewport.
        self.browser.click(210, 111)
        after = self.browser.frame(lambda i: i.tobytes() != before.tobytes())
        self.assertEqual(before.size, after.size)

    def test_font_categories_render_distinct_glyphs(self):
        self.browser.send(type='load', html='''<style>body{margin:0;background:white;color:black}div{position:absolute;left:0;font-size:32px;line-height:60px}</style>
<div style="top:0;font-family:sans-serif">Narrow iii and wide WWW</div>
<div style="top:80px;font-family:serif">Narrow iii and wide WWW</div>
<div style="top:160px;font-family:monospace">Narrow iii and wide WWW</div>''')
        frame = self.browser.frame()
        samples = [frame.crop((0, y, 1500, y+140)) for y in (0,160,320)]
        widths = []
        for sample in samples:
            box = ImageChops.invert(sample).getbbox()
            self.assertIsNotNone(box)
            widths.append(box[2]-box[0])
        self.assertEqual(len(set(widths)), 3, widths)
        print('Rendered sans/serif/mono specimen widths at 2x:', widths)

    def test_explicit_data_image_is_rendered(self):
        image = Image.new('RGB', (20,20), (0,0,255))
        data = io.BytesIO()
        image.save(data, format='PNG')
        encoded = base64.b64encode(data.getvalue()).decode()
        self.browser.send(type='load', html='<body style="margin:0"><img width="100" height="100" src="data:image/png;base64,' + encoded + '">')
        self.browser.frame(lambda i: i.getpixel((80,80)) == (0,0,255))


if __name__ == '__main__':
    unittest.main(verbosity=2)
