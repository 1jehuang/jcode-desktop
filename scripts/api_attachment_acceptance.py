"""Real harness API cwd acceptance, using only a caller-owned private runtime."""
import json
from pathlib import Path
import socket
import time


class Client:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(str(path))
        self.stream = self.socket.makefile('rwb')
        self.sequence = 0
        assert self.request(req='hello', min_version=1, max_version=1,
                            client='cwd-acceptance')['ev'] == 'hello_ok'

    def request(self, **request):
        self.sequence += 1
        self.stream.write((json.dumps({'v': 1, 'id': self.sequence, **request}) + '\n').encode())
        self.stream.flush()
        while True:
            line = self.stream.readline()
            assert line, f'API closed before replying to {request}'
            frame = json.loads(line)
            if frame.get('reply_to') == self.sequence:
                return frame

    def close(self):
        self.stream.close()
        self.socket.close()


def verify(root, env):
    """Map each changed public API outcome to a real-daemon observation."""
    clients, evidence = [], []
    api = Path(env['JCODE_API_SOCKET'])
    history = Path(env['JCODE_HOME']) / 'sessions'
    desired = root / 'home/api-project'
    desired.mkdir()

    def client():
        value = Client(api)
        clients.append(value)
        return value

    def persist(connection, session_id, marker, disconnect=False):
        deadline = time.monotonic() + 15
        while True:
            reply = connection.request(req='send_message', session_id=session_id,
                                       content=marker, no_reply=True)
            if reply['ev'] == 'ok':
                break
            assert 'session is busy' in reply.get('message', '') and time.monotonic() < deadline, reply
            time.sleep(.1)
        if disconnect:
            # A resumed session may flush context-only edits on disconnect.
            # Check the public live history first, then observe that flush.
            history_reply = connection.request(req='get_history', session_id=session_id)
            assert history_reply['ev'] == 'history', history_reply
            assert marker in json.dumps(history_reply['messages']), history_reply
            connection.close()
            clients.remove(connection)
        path = history / f'{session_id}.json'
        while time.monotonic() < deadline:
            if path.exists():
                record = json.loads(path.read_text())
                if marker in json.dumps(record['messages']):
                    return record
            time.sleep(.05)
        raise AssertionError('context-only marker was not persisted')

    def check(name, **observed):
        evidence.append({'requirement': name, 'result': 'passed', **observed})
        print(json.dumps(evidence[-1]), flush=True)

    try:
        owner = client()
        created = owner.request(req='create_session', working_dir=str(desired))
        assert created['ev'] == 'attached', created
        session_id = created['session']['session_id']
        observer = client()
        # The legacy daemon requires an authoritative cwd for initial Subscribe.
        # Verify the new fail-closed public output, including transport survival.
        missing = observer.request(req='attach_session', session_id=session_id)
        assert missing['ev'] == 'error' and missing['code'] == 'unknown_session', missing
        assert observer.request(req='ping')['ev'] == 'pong'
        check('unpersisted target fails safely without closing API connection', reply=missing)

        record = persist(owner, session_id, 'Cwd acceptance initial context')
        assert record['working_dir'] == str(desired), record['working_dir']
        check('explicit create directory reaches real persisted session', cwd=record['working_dir'])
        owner.close()
        clients.remove(owner)
        attached = observer.request(req='attach_session', session_id=session_id)
        assert attached['ev'] == 'attached', attached
        assert attached['session']['session_id'] == session_id
        assert attached['session']['working_dir'] == str(desired), attached
        check('persisted attach replies with the correct session and cwd', reply=attached)
        record = persist(observer, session_id, 'Cwd acceptance after reattach', disconnect=True)
        assert record['working_dir'] == str(desired), record['working_dir']
        check('reattached agent retains cwd rather than bridge launch directory',
              cwd=record['working_dir'], bridge_cwd=str(root))

        default_owner = client()
        created = default_owner.request(req='create_session')
        assert created['ev'] == 'attached', created
        default_id = created['session']['session_id']
        record = persist(default_owner, default_id, 'Default cwd acceptance')
        assert record['working_dir'] == str(root), record['working_dir']
        check('default create retains the established bridge-cwd behavior', cwd=record['working_dir'])

        missing_client = client()
        for target in ['session_missing_cwd_acceptance', '../../config']:
            reply = missing_client.request(req='attach_session', session_id=target)
            assert reply['ev'] == 'error' and reply['code'] == 'unknown_session', reply
            assert missing_client.request(req='ping')['ev'] == 'pong'
            check('unknown or invalid target is rejected without connection loss', target=target, reply=reply)
        (root / 'api-attachment-acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print('PASS: all mapped real API directory and error-contract checks passed.', flush=True)
    finally:
        for connection in reversed(clients):
            connection.close()
