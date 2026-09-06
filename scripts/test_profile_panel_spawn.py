import importlib.util
import json
from pathlib import Path
import socket
import tempfile
import unittest


spec = importlib.util.spec_from_file_location('profile_panel_spawn', Path(__file__).with_name('profile-panel-spawn.py'))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class PanelSpawnProfileTests(unittest.TestCase):
    def test_reads_public_navigation_and_ignores_partial_writes(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'state'
            self.assertEqual(profile.navigation(path), {})
            path.write_text('navigation={')
            self.assertEqual(profile.navigation(path), {})
            state = {'rows': [{'panels': [{'session': 'session_test', 'focused': True}]}]}
            path.write_text('strip=0\nnavigation=' + json.dumps(state) + '\n')
            self.assertEqual(profile.panels(profile.navigation(path)), state['rows'][0]['panels'])

    def test_proxy_delays_only_create_and_preserves_wire_bytes(self):
        with tempfile.TemporaryDirectory() as root:
            upstream = str(Path(root) / 'upstream')
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(upstream)
            listener.listen()
            listener.settimeout(2)
            proxy_path = Path(root) / 'proxy'
            proxy = profile.DelayedCreates(proxy_path, upstream, .2)
            client = socket.socket(socket.AF_UNIX)
            client.settimeout(2)
            try:
                client.connect(str(proxy_path))
                server, _ = listener.accept()
                with server:
                    server.settimeout(2)
                    hello = b'{"v":1,"id":1,"req":"hello"}\n'
                    client.sendall(hello)
                    self.assertEqual(server.recv(4096), hello)
                    request = b'{"v":1,"id":2,"req":"create_session","working_dir":"/test path"}\n'
                    client.sendall(request)
                    server.settimeout(.03)
                    with self.assertRaises(socket.timeout):
                        server.recv(4096)
                    server.settimeout(2)
                    self.assertEqual(server.recv(4096), request)
                    reply = b'{"v":1,"reply_to":2,"ev":"session_created"}\n'
                    server.sendall(reply)
                    self.assertEqual(client.recv(4096), reply)
            finally:
                client.close()
                proxy.close()
                listener.close()


if __name__ == '__main__':
    unittest.main()
