import base64
from datetime import datetime, timedelta, timezone
import gzip
import threading
import unittest
from unittest.mock import patch
from types import SimpleNamespace

import control
import template

KEY = 'ssh-ed25519 ' + base64.b64encode(b'\0\0\0\x0bssh-ed25519\0\0\0\x20' + bytes(32)).decode()


class TemplateTests(unittest.TestCase):
    def setUp(self):
        self.doc = template.generate(KEY)
        self.resources = self.doc['Resources']

    def test_only_one_predictable_host_and_encrypted_retained_storage(self):
        hosts = [r for r in self.resources.values() if r['Type'] == 'AWS::EC2::Instance']
        self.assertEqual(len(hosts), 1)
        host = hosts[0]
        self.assertEqual(host['Properties']['InstanceType'], 'm7i.large')
        self.assertEqual(host['DeletionPolicy'], 'Retain')
        self.assertTrue(host['Properties']['DisableApiTermination'])
        disk = host['Properties']['BlockDeviceMappings'][0]['Ebs']
        self.assertEqual(disk['VolumeSize'], 30)
        self.assertTrue(disk['Encrypted'])
        self.assertFalse(disk['DeleteOnTermination'])
        self.assertEqual(host['Properties']['InstanceInitiatedShutdownBehavior'], 'stop')

    def test_no_public_ingress_nat_or_elastic_ip(self):
        self.assertEqual(self.resources['SecurityGroup']['Properties']['SecurityGroupIngress'], [])
        self.assertFalse(any(r['Type'] in ('AWS::EC2::EIP', 'AWS::EC2::NatGateway') for r in self.resources.values()))
        self.assertEqual(self.resources['Host']['Properties']['MetadataOptions']['HttpTokens'], 'required')

    def test_host_role_cannot_provision_and_model_is_allowlisted(self):
        statements = self.resources['HostRole']['Properties']['Policies'][0]['PolicyDocument']['Statement']
        self.assertEqual(statements[0]['Action'], ['bedrock:InvokeModel', 'bedrock:InvokeModelWithResponseStream'])
        self.assertIn('amazon.nova-micro-v1:0', str(statements[0]['Resource']))
        self.assertNotIn('ec2:', str(statements))
        self.assertNotIn('iam:', str(statements))
        self.assertNotIn('sts:', str(statements))

    def test_independent_guard_cannot_start_or_terminate(self):
        statements = self.resources['GuardRole']['Properties']['Policies'][0]['PolicyDocument']['Statement']
        self.assertNotIn('ec2:StartInstances', str(statements))
        self.assertNotIn('ec2:TerminateInstances', str(statements))
        self.assertEqual(statements[1]['Action'], 'ec2:StopInstances')
        self.assertIn('${Host}', str(statements[1]['Resource']))
        self.assertEqual(self.resources['GuardSchedule']['Properties']['ScheduleExpression'], 'rate(1 minute)')
        variables = self.resources['Guard']['Properties']['Environment']['Variables']
        self.assertEqual(variables['MONTHLY_HOURS'], '50')
        self.assertEqual(variables['MAX_BOOT_HOURS'], '2')

    def test_pinned_release_and_local_fail_safe(self):
        raw = base64.b64decode(self.resources['Host']['Properties']['UserData'])
        self.assertLessEqual(len(raw), 16384)
        script = gzip.decompress(raw).decode()
        self.assertIn('OnBootSec=2h', script)
        self.assertIn('sha256sum --check -', script)
        self.assertIn('/releases/download/v0.84.0/', script)
        self.assertIn('default_model = "amazon.nova-micro-v1:0"', script)
        self.assertNotIn('__SSH_PUBLIC_KEY__', script)
        self.assertNotIn('AWS_SECRET_ACCESS_KEY', script)

    def test_public_key_injection_rejected(self):
        for key in ['', "ssh-ed25519 '$(id)'", 'ssh-rsa AAAA', 'ssh-ed25519 not_base64']:
            with self.subTest(key=key), self.assertRaises(ValueError):
                template.generate(key)
        # Ignore comments completely rather than evaluating or emitting them.
        encoded = template.generate(KEY + " $(touch /tmp/never)")['Resources']['Host']['Properties']['UserData']
        self.assertNotIn('touch /tmp/never', gzip.decompress(base64.b64decode(encoded)).decode())


class ControlTests(unittest.TestCase):
    config = {'instance_id': 'i-test', 'account_id': '123456789012', 'ledger_table': 'ledger',
              'guard_rule': 'rule', 'guard_function': 'guard'}

    def test_wrong_account_refused(self):
        with patch.object(control, 'aws', return_value={'Account': '999999999999'}):
            with self.assertRaisesRegex(RuntimeError, 'account differs'):
                control.identity(self.config)

    def test_missing_ledger_is_not_free_runtime(self):
        with patch.object(control, 'aws', return_value={}):
            with self.assertRaisesRegex(RuntimeError, 'not initialized'):
                control.ledger(self.config)

    def test_wrong_host_or_missing_tag_refused(self):
        host = {'InstanceId': 'i-test', 'Tags': []}
        with patch.object(control, 'aws', return_value={'Reservations': [{'Instances': [host]}]}):
            with self.assertRaisesRegex(RuntimeError, 'ownership tag'):
                control.instance(self.config)

    def test_exhaustion_never_calls_start(self):
        with patch.object(control, 'check_guard'), patch.object(control, 'ledger', return_value=(3000, 3000, True)), patch.object(control, 'instance', return_value={'State': {'Name': 'stopped'}}), patch.object(control, 'aws') as api:
            with self.assertRaisesRegex(RuntimeError, 'exhausted'):
                control.wake(self.config)
            api.assert_not_called()

    def test_disabled_or_stale_guard_refuses_wake(self):
        def responses(rule, checked):
            return lambda config, service, action, *args: {
                'events': {'State': rule}, 'lambda': {'State': 'Active'},
                'dynamodb': {'Item': {'checked_at': {'S': checked}}},
            }[service]
        with patch.object(control, 'aws', side_effect=responses('DISABLED', datetime.now(timezone.utc).isoformat())):
            with self.assertRaisesRegex(RuntimeError, 'unavailable'):
                control.check_guard(self.config)
        old = (datetime.now(timezone.utc) - timedelta(minutes=4)).isoformat()
        with patch.object(control, 'aws', side_effect=responses('ENABLED', old)):
            with self.assertRaisesRegex(RuntimeError, 'stale'):
                control.check_guard(self.config)

    def test_stopping_host_does_not_start(self):
        with patch.object(control, 'check_guard'), patch.object(control, 'ledger', return_value=(10, 3000, False)), patch.object(control, 'instance', return_value={'State': {'Name': 'stopping'}}), patch.object(control, 'aws') as api:
            with self.assertRaisesRegex(RuntimeError, 'still stopping'):
                control.wake(self.config)
            api.assert_not_called()

    def test_wake_does_not_trust_stale_ssm_online(self):
        with patch.object(control, 'check_guard'), patch.object(control, 'ledger', return_value=(10, 3000, False)), patch.object(control, 'instance', return_value={'State': {'Name': 'running'}}), patch.object(control, 'aws', return_value={'InstanceInformationList': [{'PingStatus': 'Online'}]}), patch.object(control.subprocess, 'run', side_effect=[SimpleNamespace(returncode=1), SimpleNamespace(returncode=0)]) as ssh, patch.object(control.time, 'sleep') as wait, patch('builtins.print'):
            control.wake(self.config)
            self.assertEqual(ssh.call_count, 2)
            wait.assert_called_once_with(4)

    def test_preflight_overlaps_all_five_reads_but_starts_only_after_checks(self):
        # Barriers prove concurrency without flaky wall-clock thresholds. A
        # serial implementation times out rather than silently passing.
        barrier = threading.Barrier(5, timeout=3)
        completed = set()
        lock = threading.Lock()
        def api(config, service, action, *args):
            if action == 'start-instances':
                self.assertEqual(len(completed), 5)
                return {}
            name = (service, action)
            if service == 'dynamodb':
                key = control.json.loads(args[args.index('--key') + 1])
                name = (service, key['sk']['S'])
                result = {'Item': {'checked_at': {'S': datetime.now(timezone.utc).isoformat()}}} if name[1] == 'STATE' else {
                    'Item': {'consumed_minutes': {'N': '10'}, 'budget_minutes': {'N': '3000'}, 'depleted': {'BOOL': False}}}
            elif service == 'events':
                result = {'State': 'ENABLED'}
            elif service == 'lambda':
                result = {'State': 'Active'}
            elif service == 'ec2':
                result = {'Reservations': [{'Instances': [{
                    'InstanceId': 'i-test', 'Tags': [{'Key': 'Project', 'Value': 'jcode-cloud-alpha'}],
                    'State': {'Name': 'stopped'},
                }]}]}
            else:
                self.fail(f'unexpected preflight call: {service}/{action}')
            barrier.wait()
            with lock:
                completed.add(name)
            return result
        with patch.object(control, 'aws', side_effect=api), patch.object(control.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), patch('builtins.print'):
            control.wake(self.config)

    def test_any_preflight_failure_prevents_start_and_ssh(self):
        for failed in ('check_guard', 'ledger', 'instance'):
            with self.subTest(failed=failed), patch.object(control, 'check_guard') as guard, patch.object(control, 'ledger', return_value=(10, 3000, False)) as ledger, patch.object(control, 'instance', return_value={'State': {'Name': 'stopped'}}) as host, patch.object(control, 'aws') as api, patch.object(control.subprocess, 'run') as ssh:
                {'check_guard': guard, 'ledger': ledger, 'instance': host}[failed].side_effect = RuntimeError('read failed')
                with self.assertRaisesRegex(RuntimeError, 'read failed'):
                    control.wake(self.config)
                api.assert_not_called()
                ssh.assert_not_called()

    def test_cold_wake_retries_actual_ssh_without_waiting_for_ssm_inventory(self):
        with patch.object(control, 'check_guard'), patch.object(control, 'ledger', return_value=(10, 3000, False)), patch.object(control, 'instance', return_value={'State': {'Name': 'stopped'}}), patch.object(control, 'aws') as api, patch.object(control.subprocess, 'run', side_effect=[control.subprocess.TimeoutExpired('ssh', 20), SimpleNamespace(returncode=1), SimpleNamespace(returncode=0)]) as ssh, patch.object(control.time, 'sleep') as wait, patch('builtins.print'):
            control.wake(self.config)
            api.assert_called_once_with(self.config, 'ec2', 'start-instances', '--instance-ids', 'i-test')
            self.assertEqual(ssh.call_count, 3)
            self.assertEqual(wait.call_count, 2)
            for call in ssh.call_args_list:
                self.assertEqual(call.args[0][-1], 'test -f /opt/jcode-alpha/ready')
                self.assertLessEqual(call.kwargs['timeout'], 20)

    def test_status_reports_original_launch_deadline_and_remaining_allowance(self):
        now = datetime(2026, 9, 18, 23, 0, tzinfo=timezone.utc)
        launch = now - timedelta(minutes=115)
        host = {'State': {'Name': 'running'}, 'InstanceType': 'm7i.large',
                'LaunchTime': launch.isoformat()}
        with patch.object(control, 'instance', return_value=host), patch.object(control, 'ledger', return_value=(120, 3000, False)):
            result = control.status(self.config, now)
        self.assertEqual(result['lease_remaining_seconds'], 300)
        self.assertEqual(result['lease_deadline'], (launch + timedelta(hours=2)).isoformat())
        self.assertEqual(result['remaining_minutes'], 2880)

    def test_wake_progress_is_flushed_and_distinguishes_running_from_starting(self):
        for state in ('running', 'stopped'):
            with self.subTest(state=state), \
                    patch.object(control, 'check_guard') as guard, \
                    patch.object(control, 'ledger', return_value=(10, 3000, False)), \
                    patch.object(control, 'instance', side_effect=[
                        {'State': {'Name': state}}, {'State': {'Name': 'running'}}]), \
                    patch.object(control, 'aws', return_value={
                        'InstanceInformationList': [{'PingStatus': 'Online'}]}) as api, \
                    patch.object(control.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), \
                    patch('builtins.print') as output:
                control.wake(self.config)
                guard.assert_called_once_with(self.config)
                phases = [call for call in output.call_args_list
                          if call.kwargs.get('file') is control.sys.stderr]
                self.assertTrue(all(call.kwargs.get('flush') is True for call in phases))
                text = '\n'.join(call.args[0] for call in phases)
                self.assertIn('guard and allowance', text)
                self.assertIn('private SSM', text)
                self.assertIn('Verifying SSH', text)
                self.assertIn('already running' if state == 'running' else 'Starting the shared', text)
                starts = [call for call in api.call_args_list if call.args[2] == 'start-instances']
                self.assertEqual(len(starts), int(state == 'stopped'))

    def test_stopped_status_has_no_running_lease(self):
        host = {'State': {'Name': 'stopped'}, 'InstanceType': 'm7i.large'}
        with patch.object(control, 'instance', return_value=host), patch.object(control, 'ledger', return_value=(3010, 3000, True)):
            result = control.status(self.config)
        self.assertIsNone(result['lease_remaining_seconds'])
        self.assertIsNone(result['lease_deadline'])
        self.assertEqual(result['remaining_minutes'], 0)

    def test_expired_lease_does_not_display_negative_time(self):
        now = datetime.now(timezone.utc)
        host = {'State': {'Name': 'stopping'}, 'InstanceType': 'm7i.large',
                'LaunchTime': (now - timedelta(hours=3)).isoformat()}
        with patch.object(control, 'instance', return_value=host), patch.object(control, 'ledger', return_value=(20, 3000, False)):
            self.assertEqual(control.status(self.config, now)['lease_remaining_seconds'], 0)

    def test_status_refuses_future_or_naive_launch(self):
        now = datetime.now(timezone.utc)
        for launch in (now + timedelta(minutes=5), now.replace(tzinfo=None)):
            host = {'State': {'Name': 'running'}, 'InstanceType': 'm7i.large',
                    'LaunchTime': launch.isoformat()}
            with patch.object(control, 'instance', return_value=host), patch.object(control, 'ledger', return_value=(20, 3000, False)):
                with self.assertRaisesRegex(RuntimeError, 'launch time'):
                    control.status(self.config, now)


if __name__ == '__main__':
    unittest.main()
