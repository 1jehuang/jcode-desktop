#!/usr/bin/env python3
"""Install a local alias for an existing alpha stack, pinning its host key via SSM."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

import control


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', default='jcode-personal')
    parser.add_argument('--account-id', required=True)
    args = parser.parse_args()
    config = {'profile': args.profile, 'region': 'us-east-1', 'account_id': args.account_id}
    control.identity(config)
    stack = control.aws(config, 'cloudformation', 'describe-stacks', '--stack-name', 'jcode-cloud-alpha')['Stacks'][0]
    if stack['StackStatus'] not in ('CREATE_COMPLETE', 'UPDATE_COMPLETE'):
        raise RuntimeError('Wait for a successful stack deployment before local setup')
    outputs = {o['OutputKey']: o['OutputValue'] for o in stack['Outputs']}
    config.update(instance_id=outputs['InstanceId'], ledger_table=outputs['LedgerTable'],
                  guard_function=outputs['GuardFunction'], guard_rule=outputs['GuardRule'])
    control.instance(config)
    command = control.aws(config, 'ssm', 'send-command', '--instance-ids', config['instance_id'],
                          '--document-name', 'AWS-RunShellScript', '--parameters', json.dumps({'commands': [
                              'test -f /opt/jcode-alpha/ready && cat /etc/ssh/ssh_host_ed25519_key.pub']}),
                          '--comment', 'Read alpha host public key over authenticated SSM')['Command']['CommandId']
    result = None
    for _ in range(30):
        time.sleep(2)
        try:
            result = control.aws(config, 'ssm', 'get-command-invocation', '--command-id', command,
                                 '--instance-id', config['instance_id'])
        except RuntimeError as error:
            if 'InvocationDoesNotExist' in str(error):
                continue
            raise
        if result['Status'] not in ('Pending', 'InProgress', 'Delayed'):
            break
    if result is None or result['Status'] != 'Success':
        raise RuntimeError('Host bootstrap is not ready. No SSH configuration was changed.')
    parts = result['StandardOutputContent'].strip().split()
    if len(parts) < 2 or parts[0] != 'ssh-ed25519' or not re.fullmatch(r'[A-Za-z0-9+/]+={0,2}', parts[1]):
        raise RuntimeError('SSM returned an invalid host public key')
    home = Path.home()
    key = home / '.ssh/jcode_cloud_alpha'
    if not key.is_file():
        raise RuntimeError('Missing dedicated SSH private key. Do not overwrite or regenerate a deployed key.')
    os.umask(0o077)
    config_dir = home / '.config/jcode'
    config_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    bin_dir = home / '.local/bin'
    bin_dir.mkdir(parents=True, exist_ok=True)
    helper = bin_dir / 'jcode-cloud-alpha'
    source = Path(__file__).with_name('control.py').resolve()
    if helper.exists() or helper.is_symlink():
        if not helper.is_symlink() or helper.resolve() != source:
            raise RuntimeError('Refusing to replace an unrelated local helper')
    else:
        helper.symlink_to(source)
    source.chmod(0o755)
    known = home / '.ssh/jcode_cloud_alpha_known_hosts'
    known_text = 'jcode-cloud-alpha ' + ' '.join(parts[:2]) + '\n'
    if known.exists() and known.read_text() != known_text:
        raise RuntimeError('Pinned host key changed. Investigate replacement before re-pinning.')
    known.write_text(known_text)
    known.chmod(0o600)
    control.CONFIG.write_text(json.dumps(config, indent=2) + '\n')
    control.CONFIG.chmod(0o600)
    ssh_config = home / '.ssh/config'
    before = ssh_config.read_text() if ssh_config.exists() else ''
    if re.search(r'(?mi)^Host\s+.*\bjcode-cloud-alpha\b', before):
        print('Existing alpha SSH alias preserved.')
    else:
        # Put exact alias first so a user's existing wildcard defaults cannot weaken it.
        block = f'''# Jcode personal cloud alpha, installed after SSM host-key verification.
Host jcode-cloud-alpha
    HostName {config['instance_id']}
    User ec2-user
    IdentityFile {key}
    IdentitiesOnly yes
    StrictHostKeyChecking yes
    HostKeyAlias jcode-cloud-alpha
    UserKnownHostsFile {known}
    ProxyCommand {helper} proxy
    ServerAliveInterval 30
    ServerAliveCountMax 3

'''
        ssh_config.write_text(block + before)
        ssh_config.chmod(0o600)
    subprocess.run(['ssh', '-o', 'BatchMode=yes', 'jcode-cloud-alpha',
                    'test -f /opt/jcode-alpha/ready && ~/.local/bin/jcode --version'], check=True, timeout=45)
    print('Installed. Open Desktop → Machines → jcode-cloud-alpha → Connect.')


if __name__ == '__main__':
    main()
