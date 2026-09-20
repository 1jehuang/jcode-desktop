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
        with patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)), patch.object(control, 'ledger', return_value=(3000, 3000, True)), patch.object(control, 'instance', return_value={'State': {'Name': 'stopped'}}), patch.object(control, 'aws') as api:
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
        with patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)), patch.object(control, 'ledger', return_value=(10, 3000, False)), patch.object(control, 'instance', return_value={'State': {'Name': 'stopping'}}), patch.object(control, 'aws') as api:
            with self.assertRaisesRegex(RuntimeError, 'still stopping'):
                control.wake(self.config)
            api.assert_not_called()

    def test_wake_does_not_trust_stale_ssm_online(self):
        with patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)), patch.object(control, 'ledger', return_value=(10, 3000, False)), patch.object(control, 'instance', return_value={'State': {'Name': 'running'}}), patch.object(control, 'aws', return_value={'InstanceInformationList': [{'PingStatus': 'Online'}]}), patch.object(control.subprocess, 'run', side_effect=[SimpleNamespace(returncode=1), SimpleNamespace(returncode=0)]) as ssh, patch.object(control.time, 'sleep') as wait, patch('builtins.print'):
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
            with self.subTest(failed=failed), patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)) as guard, patch.object(control, 'ledger', return_value=(10, 3000, False)) as ledger, patch.object(control, 'instance', return_value={'State': {'Name': 'stopped'}}) as host, patch.object(control, 'aws') as api, patch.object(control.subprocess, 'run') as ssh:
                {'check_guard': guard, 'ledger': ledger, 'instance': host}[failed].side_effect = RuntimeError('read failed')
                with self.assertRaisesRegex(RuntimeError, 'read failed'):
                    control.wake(self.config)
                api.assert_not_called()
                ssh.assert_not_called()

    def test_guard_progress_waits_for_identity_success(self):
        import io
        from contextlib import redirect_stderr, redirect_stdout
        for succeeds in (True, False):
            with self.subTest(succeeds=succeeds):
                output = io.StringIO()
                reads_done = threading.Event()

                def account(config):
                    # A concurrent host read gates identity completion.
                    self.assertTrue(reads_done.wait(timeout=3))
                    self.assertIn('Checking AWS sign-in and cloud safety...', output.getvalue())
                    self.assertNotIn('Checking cloud runtime guard and allowance...', output.getvalue())
                    if not succeeds:
                        raise RuntimeError('identity failed')

                def host(config):
                    reads_done.set()
                    return {'State': {'Name': 'running'}}

                with patch.object(control, 'identity', side_effect=account), \
                        patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)), \
                        patch.object(control, 'ledger', return_value=(10, 3000, False)), \
                        patch.object(control, 'instance', side_effect=host), \
                        patch.object(control, 'aws') as api, \
                        patch.object(control.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as ssh, \
                        redirect_stderr(output), redirect_stdout(io.StringIO()):
                    if succeeds:
                        control.wake(self.config, verify_identity=True)
                        self.assertIn('Checking cloud runtime guard and allowance...', output.getvalue())
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'identity failed'):
                            control.wake(self.config, verify_identity=True)
                        self.assertNotIn('Checking cloud runtime guard and allowance...', output.getvalue())
                        ssh.assert_not_called()
                    api.assert_not_called()

    def test_guard_expiring_while_identity_or_ledger_finishes_prevents_side_effects(self):
        for delayed in ('identity', 'ledger'):
            for state in ('stopped', 'running'):
                with self.subTest(delayed=delayed, state=state):
                    start = datetime.now(timezone.utc)
                    guard_done = threading.Event()

                    def guard(config):
                        guard_done.set()
                        return start - timedelta(seconds=179)

                    def slow_read(config):
                        self.assertTrue(guard_done.wait(timeout=3))
                        clock.now.return_value = start + timedelta(seconds=2)
                        return (10, 3000, False) if delayed == 'ledger' else None

                    with patch.object(control, 'datetime', wraps=datetime) as clock, \
                            patch.object(control, 'check_guard', side_effect=guard), \
                            patch.object(control, 'identity') as account, \
                            patch.object(control, 'ledger', return_value=(10, 3000, False)) as usage, \
                            patch.object(control, 'instance', return_value={'State': {'Name': state}}), \
                            patch.object(control, 'aws') as api, \
                            patch.object(control.subprocess, 'run') as ssh:
                        clock.now.return_value = start
                        (account if delayed == 'identity' else usage).side_effect = slow_read
                        with self.assertRaisesRegex(RuntimeError, 'heartbeat is stale'):
                            control.wake(self.config, verify_identity=True)
                        api.assert_not_called()
                        ssh.assert_not_called()

    def test_cold_wake_retries_actual_ssh_without_waiting_for_ssm_inventory(self):
        with patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)), patch.object(control, 'ledger', return_value=(10, 3000, False)), patch.object(control, 'instance', return_value={'State': {'Name': 'stopped'}}), patch.object(control, 'aws') as api, patch.object(control.subprocess, 'run', side_effect=[control.subprocess.TimeoutExpired('ssh', 20), SimpleNamespace(returncode=1), SimpleNamespace(returncode=0)]) as ssh, patch.object(control.time, 'sleep') as wait, patch('builtins.print'):
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
                    patch.object(control, 'check_guard', return_value=datetime.now(timezone.utc)) as guard, \
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
                if state == 'running':
                    self.assertIn('private SSM', text)
                    self.assertIn('Verifying SSH', text)
                else:
                    self.assertIn('Waiting for shared cloud VM to become reachable', text)
                    self.assertIn('Shared cloud VM is running. SSH bootstrap ready.', text)
                    self.assertNotIn('private SSM', text)
                    self.assertNotIn('Verifying SSH', text)
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


class ReadinessTests(unittest.TestCase):
    config = ControlTests.config

    def setUp(self):
        self.now = datetime.now(timezone.utc)
        self.host = {'InstanceId': 'i-test',
                     'Tags': [{'Key': 'Project', 'Value': 'jcode-cloud-alpha'}],
                     'State': {'Name': 'running'},
                     'LaunchTime': (self.now - timedelta(minutes=30)).isoformat()}
        self.account = self.config['account_id']
        self.rule, self.function = 'ENABLED', 'Active'
        self.checked = self.now.isoformat()
        self.used, self.budget, self.depleted = '10', '3000', False
        self.barrier = None

    def api(self, config, service, action, *args):
        # Deliberate allowlist: any mutation, SSM session, or unexpected call fails.
        if (service, action) == ('sts', 'get-caller-identity'):
            result = {'Account': self.account}
        elif (service, action) == ('events', 'describe-rule'):
            result = {'State': self.rule}
        elif (service, action) == ('lambda', 'get-function-configuration'):
            result = {'State': self.function}
        elif (service, action) == ('ec2', 'describe-instances'):
            result = {'Reservations': [{'Instances': [self.host]}]}
        elif (service, action) == ('dynamodb', 'get-item'):
            self.assertIn('--consistent-read', args)
            key = control.json.loads(args[args.index('--key') + 1])
            result = {'Item': {'checked_at': {'S': self.checked}}} if key['sk']['S'] == 'STATE' else {
                'Item': {'consumed_minutes': {'N': self.used},
                         'budget_minutes': {'N': self.budget}, 'depleted': {'BOOL': self.depleted}}}
        else:
            self.fail(f'non-read-only operation: {service}/{action}')
        if self.barrier:
            self.barrier.wait()
        return result

    def check(self):
        with patch.object(control, 'aws', side_effect=self.api) as api, \
                patch.object(control.model_sync, 'sync_personal_alpha') as sync, \
                patch.object(control.subprocess, 'run') as subprocess, \
                patch.object(control.os, 'execvp') as execvp, \
                patch.object(control.Path, 'read_text') as file_read:
            try:
                return control.check_ready(self.config, self.now)
            finally:
                self.assertEqual(api.call_count, 6)
                sync.assert_not_called()
                subprocess.assert_not_called()
                execvp.assert_not_called()
                file_read.assert_not_called()

    def test_receipt_and_all_six_checks_overlap(self):
        self.barrier = threading.Barrier(6, timeout=3)
        receipt = self.check()
        self.assertEqual(receipt, {
            'instance_id': 'i-test', 'state': 'running',
            'launch_time': self.host['LaunchTime'],
            'lease_deadline': int((self.now + timedelta(minutes=90)).timestamp()),
            'lease_remaining_seconds': 5400, 'allowance_remaining_minutes': 2990,
            'observed_at': int(self.now.timestamp()), 'valid_for_seconds': 30,
            'model_sync_supported': True})

    def test_validity_is_bounded_by_guard_freshness_and_lease(self):
        self.checked = (self.now - timedelta(seconds=170)).isoformat()
        self.assertEqual(self.check()['valid_for_seconds'], 10)
        self.host['LaunchTime'] = (self.now - timedelta(hours=2) + timedelta(seconds=5)).isoformat()
        self.assertEqual(self.check()['valid_for_seconds'], 5)

    def test_guard_expiring_during_other_reads_fails_closed(self):
        with patch.object(control, 'check_guard', return_value=self.now - timedelta(seconds=181)), \
                patch.object(control, 'aws', side_effect=self.api):
            with self.assertRaisesRegex(RuntimeError, 'stale'):
                control.check_ready(self.config, self.now)

    def test_canonical_aws_launch_string_is_preserved(self):
        self.host['LaunchTime'] = self.host['LaunchTime'].replace('+00:00', 'Z')
        self.assertEqual(self.check()['launch_time'], self.host['LaunchTime'])

    def test_wrong_identity_and_disabled_or_stale_guard_fail_closed(self):
        for attr, value, message in (
                ('account', '999999999999', 'account differs'),
                ('rule', 'DISABLED', 'unavailable'),
                ('function', 'Inactive', 'unavailable'),
                ('checked', (self.now - timedelta(minutes=4)).isoformat(), 'stale'),
                ('checked', (self.now + timedelta(minutes=1)).isoformat(), 'stale')):
            with self.subTest(attr=attr, value=value):
                self.setUp()
                setattr(self, attr, value)
                with self.assertRaisesRegex(RuntimeError, message):
                    self.check()

    def test_all_nonrunning_states_fail_closed_without_wake(self):
        for state in ('stopped', 'stopping', 'pending', 'shutting-down', 'terminated', 'unknown'):
            with self.subTest(state=state):
                self.host['State']['Name'] = state
                with self.assertRaisesRegex(RuntimeError, 'not running'):
                    self.check()

    def test_wrong_instance_and_missing_ownership_fail_closed(self):
        self.host['InstanceId'] = 'i-other'
        with self.assertRaisesRegex(RuntimeError, 'configured host'):
            self.check()
        self.host['InstanceId'] = 'i-test'
        self.host['Tags'] = []
        with self.assertRaisesRegex(RuntimeError, 'ownership tag'):
            self.check()

    def test_expired_invalid_and_near_expiry_lease_fail_closed(self):
        for launch in (self.now - timedelta(hours=3), self.now - timedelta(hours=2),
                       self.now - timedelta(hours=2) + timedelta(milliseconds=500),
                       self.now + timedelta(seconds=1), self.now.replace(tzinfo=None)):
            with self.subTest(launch=launch):
                self.host['LaunchTime'] = launch.isoformat()
                with self.assertRaises(RuntimeError):
                    self.check()
        self.host['LaunchTime'] = 'not-a-date'
        with self.assertRaises(ValueError):
            self.check()

    def test_allowance_exhausted_depleted_or_invalid_fails_closed(self):
        for used, budget, depleted in (('3000', '3000', False), ('3001', '3000', False),
                                      ('10', '3000', True), ('-1', '3000', False),
                                      ('0', '0', False)):
            with self.subTest(used=used, budget=budget, depleted=depleted):
                self.used, self.budget, self.depleted = used, budget, depleted
                with self.assertRaises(RuntimeError):
                    self.check()

    def test_any_read_failure_propagates_without_receipt(self):
        for name in ('identity', 'check_guard', 'ledger', 'instance'):
            with self.subTest(name=name), patch.object(control, name, side_effect=RuntimeError('read failed')), \
                    patch.object(control, 'aws', side_effect=self.api), \
                    patch.object(control.subprocess, 'run') as process:
                with self.assertRaisesRegex(RuntimeError, 'read failed'):
                    control.check_ready(self.config)
                process.assert_not_called()

    def test_cli_emits_only_json_without_duplicate_identity_check(self):
        import io
        from contextlib import redirect_stdout
        out = io.StringIO()
        with patch.object(control.sys, 'argv', ['control.py', 'check-ready']), \
                patch.object(control.Path, 'read_text', return_value=control.json.dumps(self.config)), \
                patch.object(control, 'aws', side_effect=self.api) as api, redirect_stdout(out):
            control.main()
        receipt = control.json.loads(out.getvalue())
        self.assertEqual(receipt['instance_id'], 'i-test')
        self.assertEqual(api.call_count, 6)


class WakeReadyTests(unittest.TestCase):
    config = ReadinessTests.config
    setUp = ReadinessTests.setUp
    api = ReadinessTests.api

    # Exercise the real wake and final proof with an allowlisted fake AWS API.
    def run_cli(self, action='wake-ready', after_boot=None, initial_failure=None, sync_failure=None):
        import io
        from contextlib import redirect_stdout, redirect_stderr
        self.stdout, self.stderr = io.StringIO(), io.StringIO()
        self.starts = 0
        self.reads = []
        self.booted = False
        self.sync_calls = 0
        self.preflight_barrier = threading.Barrier(6, timeout=3)
        lock = threading.Lock()

        def api(config, service, operation, *args):
            if operation == 'start-instances':
                self.assertEqual(len(self.reads), 6)
                self.assertFalse(self.booted)
                self.starts += 1
                self.host['State']['Name'] = 'running'
                return {}
            result = self.api(config, service, operation, *args)
            if not self.booted:
                self.preflight_barrier.wait()
            with lock:
                self.reads.append((service, operation))
            return result

        def ssh(*args, **kwargs):
            self.assertEqual(len(self.reads), 6)
            if self.starts or self.host['State']['Name'] == 'pending':
                self.assertNotIn('private SSM', self.stderr.getvalue())
                self.assertNotIn('Verifying SSH', self.stderr.getvalue())
                self.assertNotIn('SSH bootstrap ready', self.stderr.getvalue())
            self.booted = True
            if after_boot:
                after_boot()
            return SimpleNamespace(returncode=0, stdout='SECRET', stderr='SECRET')

        def synchronize(config):
            self.assertTrue(self.booted, 'never transfer credentials before bootstrap')
            self.assertEqual(len(self.reads), 6, 'final safety receipt must follow synchronization')
            self.assertEqual(config, self.config)
            self.assertEqual(self.stdout.getvalue(), '', 'never report ready before synchronization')
            self.sync_calls += 1
            if sync_failure:
                raise sync_failure

        def interactive_ssh(*args):
            self.assertEqual(self.sync_calls, 1, 'SSH shell requires successful model sync')

        if initial_failure:
            initial_failure()
        with patch.object(control.sys, 'argv', ['control.py', action]), \
                patch.object(control.Path, 'read_text', return_value=control.json.dumps(self.config)), \
                patch.object(control, 'aws', side_effect=api), \
                patch.object(control.model_sync, 'sync_personal_alpha', side_effect=synchronize), \
                patch.object(control.os, 'execvp', side_effect=interactive_ssh) as shell, \
                patch.object(control.subprocess, 'run', side_effect=ssh) as process, \
                redirect_stdout(self.stdout), redirect_stderr(self.stderr):
            try:
                control.main()
            finally:
                self.ssh_calls = process.call_count
                self.shell_calls = shell.call_count
                self.assertNotIn('SECRET', self.stdout.getvalue() + self.stderr.getvalue())

    def test_wake_ready_json_and_fresh_six_read_proof_without_extra_start(self):
        for state in ('stopped', 'running', 'pending'):
            with self.subTest(state=state):
                self.setUp()
                self.host['State']['Name'] = state
                self.run_cli(after_boot=lambda: self.host['State'].update(Name='running'))
                result = control.json.loads(self.stdout.getvalue())
                self.assertEqual(result['instance_id'], 'i-test')
                self.assertEqual(len(self.reads), 12)
                self.assertEqual(self.starts, int(state == 'stopped'))
                self.assertEqual(self.ssh_calls, 1)
                self.assertEqual(self.sync_calls, 1)
                self.assertTrue(result['model_sync_supported'])
                self.assertTrue(0 < result['valid_for_seconds'] <= 30)
                self.assertIsInstance(result['observed_at'], int)
                self.assertIsInstance(result['lease_deadline'], int)
                tokens = ['guard and allowance', 'Confirming cloud safety checks']
                tokens += (['private SSM', 'Verifying SSH'] if state == 'running' else
                           ['Waiting for shared cloud VM to become reachable',
                            'Shared cloud VM is running. SSH bootstrap ready.'])
                for token in tokens:
                    self.assertIn(token, self.stderr.getvalue())

    def test_legacy_wake_cli_keeps_human_output_and_single_preflight(self):
        self.run_cli(action='wake')
        self.assertIn('Ready. Desktop', self.stdout.getvalue())
        self.assertEqual(len(self.reads), 6)
        self.assertEqual(self.sync_calls, 1)

    def test_ssh_synchronizes_before_opening_interactive_shell(self):
        self.run_cli(action='ssh')
        self.assertEqual(self.sync_calls, 1)
        self.assertEqual(self.shell_calls, 1)

    def test_sync_failure_blocks_ready_receipt_and_interactive_shell(self):
        for action in ('wake', 'ssh', 'wake-ready'):
            with self.subTest(action=action):
                self.setUp()
                with self.assertRaisesRegex(control.model_sync.SyncError, 'model sync failed'):
                    self.run_cli(action=action, sync_failure=control.model_sync.SyncError('model sync failed'))
                self.assertEqual(self.sync_calls, 1)
                self.assertEqual(self.shell_calls, 0)
                self.assertEqual(self.stdout.getvalue(), '')
                self.assertEqual(len(self.reads), 6)

    def test_final_proof_fails_closed_after_long_boot(self):
        for attr, value in (
                ('checked', (self.now - timedelta(minutes=4)).isoformat()),
                ('used', '3000'), ('depleted', True), ('rule', 'DISABLED'),
                ('account', 'wrong')):
            with self.subTest(attr=attr):
                self.setUp()
                with self.assertRaises(RuntimeError):
                    self.run_cli(after_boot=lambda: setattr(self, attr, value))
                self.assertEqual(self.stdout.getvalue(), '')
                self.assertEqual(self.starts, 0)
                self.assertEqual(len(self.reads), 12)
        self.setUp()
        with self.assertRaisesRegex(RuntimeError, 'lease expired'):
            self.run_cli(after_boot=lambda: self.host.update(
                LaunchTime=(self.now - timedelta(hours=3)).isoformat()))
        self.assertEqual(self.stdout.getvalue(), '')
        self.assertEqual(self.starts, 0)

    def test_bootstrap_failure_never_obtains_or_emits_receipt(self):
        import io
        from contextlib import redirect_stdout
        out = io.StringIO()
        with patch.object(control, 'wake', side_effect=RuntimeError('bootstrap timeout')), \
                patch.object(control, 'check_ready') as check, redirect_stdout(out):
            with self.assertRaisesRegex(RuntimeError, 'bootstrap timeout'):
                control.wake_ready(self.config)
        check.assert_not_called()
        self.assertEqual(out.getvalue(), '')

    def test_initial_failure_never_starts_or_connects(self):
        for attr, value in (('account', 'wrong'), ('rule', 'DISABLED'),
                            ('used', '3000'), ('checked', (self.now - timedelta(minutes=4)).isoformat())):
            with self.subTest(attr=attr):
                self.setUp()
                self.host['State']['Name'] = 'stopped'
                with self.assertRaises(RuntimeError):
                    self.run_cli(initial_failure=lambda: setattr(self, attr, value))
                self.assertEqual(self.starts, 0)
                self.assertEqual(self.ssh_calls, 0)
                self.assertEqual(self.sync_calls, 0)
                self.assertEqual(self.stdout.getvalue(), '')


class ModelSyncCommandTests(unittest.TestCase):
    def test_explicit_sync_models_does_not_wake_or_duplicate_identity_checks(self):
        import io
        from contextlib import redirect_stdout, redirect_stderr
        config = ReadinessTests.config
        out, err = io.StringIO(), io.StringIO()
        with patch.object(control.sys, 'argv', ['control.py', 'sync-models']), \
                patch.object(control.Path, 'read_text', return_value=control.json.dumps(config)), \
                patch.object(control.model_sync, 'sync_personal_alpha', return_value=False) as sync, \
                patch.object(control, 'wake') as wake, \
                patch.object(control, 'aws') as aws, \
                patch.object(control.os, 'execvp') as shell, \
                redirect_stdout(out), redirect_stderr(err):
            control.main()
        sync.assert_called_once_with(config)
        wake.assert_not_called()
        aws.assert_not_called()
        shell.assert_not_called()
        self.assertEqual(out.getvalue(), '')
        self.assertIn('Synchronizing personal-cloud model access', err.getvalue())

    def test_explicit_sync_models_propagates_failure(self):
        config = ReadinessTests.config
        with patch.object(control.sys, 'argv', ['control.py', 'sync-models']), \
                patch.object(control.Path, 'read_text', return_value=control.json.dumps(config)), \
                patch.object(control.model_sync, 'sync_personal_alpha',
                             side_effect=control.model_sync.SyncError('sync failed')), \
                patch.object(control, 'wake') as wake:
            with self.assertRaisesRegex(control.model_sync.SyncError, 'sync failed'):
                control.main()
        wake.assert_not_called()


if __name__ == '__main__':
    unittest.main()
