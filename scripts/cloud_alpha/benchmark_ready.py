#!/usr/bin/env python3
"""Time an already-running alpha's guarded readiness without lifecycle mutations.

No inference, credential export, start, stop, or reboot. Outputs timings only.
Cold boots must be measured separately when stopping the VM is safe.
"""
import json
import threading
import time

import control


def main():
    config = json.loads(control.CONFIG.read_text())
    original_aws, original_run = control.aws, control.subprocess.run
    samples = []
    lock = threading.Lock()

    def record(phase, started):
        with lock:
            samples.append({'phase': phase, 'seconds': round(time.monotonic() - started, 3)})

    def aws(config, service, action, *args, **kwargs):
        if (service, action) not in {
            ('sts', 'get-caller-identity'), ('events', 'describe-rule'),
            ('lambda', 'get-function-configuration'), ('dynamodb', 'get-item'),
            ('ec2', 'describe-instances'), ('ssm', 'describe-instance-information'),
        }:
            raise RuntimeError('Readiness benchmark refuses cloud mutations')
        started = time.monotonic()
        try:
            return original_aws(config, service, action, *args, **kwargs)
        finally:
            record(f'{service}/{action}', started)

    def run(command, *args, **kwargs):
        if command[0] != 'ssh':
            return original_run(command, *args, **kwargs)
        if command[-1] != 'test -f /opt/jcode-alpha/ready':
            raise RuntimeError('Readiness benchmark permits only the bootstrap probe')
        started = time.monotonic()
        try:
            return original_run(command, *args, **kwargs)
        finally:
            record('ssh/bootstrap', started)

    control.aws, control.subprocess.run = aws, run
    started = time.monotonic()
    try:
        control.identity(config)
        control.wake(config)
    finally:
        control.aws, control.subprocess.run = original_aws, original_run
    print(json.dumps({'total_seconds': round(time.monotonic() - started, 3), 'phases': samples}, indent=2))


if __name__ == '__main__':
    main()
