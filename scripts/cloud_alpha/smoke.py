#!/usr/bin/env python3
"""Exercise the real remote harness used by Desktop. A model call is explicit."""
import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import time


class Harness:
    def __init__(self, host):
        self.process = subprocess.Popen(['ssh', '-T', '-o', 'BatchMode=yes', host,
                                         '~/.local/bin/jcode api --stdio'],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = bytearray()
        self.next_id = 0
        try:
            self.request('hello', min_version=1, max_version=1, client='cloud-alpha-acceptance')
        except Exception:
            self.close()
            raise

    def send(self, request, **fields):
        self.next_id += 1
        self.process.stdin.write((json.dumps({'v': 1, 'id': self.next_id, 'req': request, **fields}) + '\n').encode())
        self.process.stdin.flush()
        return self.next_id

    def event(self, timeout=90):
        deadline = time.monotonic() + timeout
        while b'\n' not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise TimeoutError('Timed out waiting for remote harness')
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data:
                raise RuntimeError('Remote harness disconnected')
            self.buffer.extend(data)
            if len(self.buffer) > 8 * 1024 * 1024:
                raise RuntimeError('Oversized harness frame')
        line, _, rest = self.buffer.partition(b'\n')
        self.buffer = bytearray(rest)
        frame = json.loads(line)
        if frame.get('ev') == 'error':
            raise RuntimeError(frame.get('message', str(frame)))
        return frame

    def request(self, request, **fields):
        request_id = self.send(request, **fields)
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            frame = self.event(max(0.1, deadline - time.monotonic()))
            if frame.get('reply_to') == request_id:
                return frame
        raise TimeoutError('No correlated reply')

    def close(self):
        try:
            self.process.stdin.close()
        except BrokenPipeError:
            pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=5)
        self.selector.close()
        self.process.stdout.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', default='jcode-cloud-alpha')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model-call', action='store_true', help='Explicitly run a low-cost remote model/tool smoke test')
    parser.add_argument('--verify-history', action='store_true', help='Reattach sessions recorded in the output file after stop/start')
    args = parser.parse_args()
    clients = []
    try:
        if args.verify_history:
            evidence = json.loads(args.output.read_text())
            for session_id in evidence['sessions']:
                client = Harness(args.host)
                clients.append(client)
                attached = client.request('attach_session', session_id=session_id)
                assert attached['session']['session_id'] == session_id
                history = client.request('get_history', session_id=session_id)
                assert history['ev'] == 'history'
                if session_id == evidence.get('model_session'):
                    assert any('JCODE_CLOUD_ALPHA_OK' in json.dumps(m) for m in history['messages'])
            evidence['history_after_reconnect'] = True
        else:
            for _ in range(2):
                clients.append(Harness(args.host))
            sessions = [c.request('create_session', working_dir='/home/ec2-user/workspaces')['session']['session_id'] for c in clients]
            assert len(set(sessions)) == 2
            evidence = {'host': args.host, 'sessions': sessions, 'distinct_sessions': True}
            if args.model_call:
                # Untouched sessions are transient. Finish a real second-session
                # turn before testing persistence. This checks ordinary saved
                # conversations, not separate context-injection semantics.
                clients[1].send('send_message', session_id=sessions[1],
                                content='Reply with exactly READY. Do not call any tools.')
                seed_deadline = time.monotonic() + 90
                while True:
                    seed = clients[1].event(max(0.1, seed_deadline - time.monotonic()))
                    if seed.get('ev') == 'turn_done':
                        break
                    if time.monotonic() >= seed_deadline:
                        raise TimeoutError('Second session seed turn did not finish')
                client, session_id = clients[0], sessions[0]
                client.send('send_message', session_id=session_id, content=(
                    'This is an authorized smoke test on my new cloud VM. Use the bash tool to run '
                    '`uname -s; pwd; printf JCODE_CLOUD_ALPHA_OK > /home/ec2-user/workspaces/cloud-alpha-smoke.txt`. '
                    'Then read that file and report its contents. Do not do anything else.'))
                deadline = time.monotonic() + 180
                tool_calls, text, done = [], '', False
                while time.monotonic() < deadline:
                    frame = client.event(max(0.1, deadline - time.monotonic()))
                    if frame.get('ev') == 'tool_done':
                        tool_calls.append(frame.get('name'))
                    if frame.get('ev') == 'text_delta':
                        text += frame.get('text', '')
                    if frame.get('ev') == 'turn_done':
                        done = True
                        break
                if not done or not tool_calls or 'JCODE_CLOUD_ALPHA_OK' not in text:
                    raise RuntimeError(f'Model/tool acceptance failed: done={done}, tools={tool_calls}, text={text}')
                evidence.update(model_session=session_id, model_turn_completed=True, tools=tool_calls, response=text)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, indent=2) + '\n')
        print(json.dumps(evidence, indent=2))
    finally:
        for client in clients:
            client.close()


if __name__ == '__main__':
    main()
