import base64
from datetime import datetime, timedelta, timezone
import gzip
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
        with patch.object(control, 'check_guard'), patch.object(control, 'ledger', return_value=(3000, 3000, True)), patch.object(control, 'aws') as api:
            with self.assertRaisesRegex(RuntimeError, 'exhausted'):
                control.wake(self.config)
            api.assert_not_called()

    def test_disabled_or_stale_guard_refuses_wake(self):
        with patch.object(control, 'aws', side_effect=[{'State': 'DISABLED'}, {'State': 'Active'}]):
            with self.assertRaisesRegex(RuntimeError, 'unavailable'):
                control.check_guard(self.config)
        old = (datetime.now(timezone.utc) - timedelta(minutes=4)).isoformat()
        with patch.object(control, 'aws', side_effect=[{'State': 'ENABLED'}, {'State': 'Active'}, {'Item': {'checked_at': {'S': old}}}]):
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


if __name__ == '__main__':
    unittest.main()
