import base64
import gzip
from pathlib import Path
import subprocess
import unittest

import template

KEY = 'ssh-ed25519 ' + base64.b64encode(b'\0\0\0\x0bssh-ed25519\0\0\0\x20' + bytes(32)).decode()


class KernelSafetyTests(unittest.TestCase):
    def setUp(self):
        self.doc = template.generate(KEY)
        self.script = gzip.decompress(base64.b64decode(
            self.doc['Resources']['Host']['Properties']['UserData'])).decode()

    def heredoc(self, path):
        return self.script.split("cat > " + path + " <<'EOF'\n", 1)[1].split('\nEOF', 1)[0]

    def test_reviewed_image_default_and_gate_before_access(self):
        self.assertEqual(self.doc['Parameters']['ImageId']['Default'], 'ami-0c230065b91270bc6')
        self.assertNotIn('__KERNEL_GUARD__', self.script)
        gate = self.script.index('/opt/jcode-alpha/check-kernel.sh\n')
        self.assertLess(gate, self.script.index('/home/ec2-user/.ssh/authorized_keys'))
        self.assertLess(gate, self.script.index('touch /opt/jcode-alpha/ready'))
        self.assertIn("trap 'systemctl poweroff' ERR", self.script)

    def test_kernel_gate_branch_floors_and_unknown_platforms(self):
        gate = self.heredoc('/opt/jcode-alpha/check-kernel.sh')
        cases = [
            ('amzn', '2023', '6.18.56-125.164.amzn2023.x86_64', True),
            ('amzn', '2023', '6.18.57-125.165.amzn2023.x86_64', True),
            ('amzn', '2023', '6.18.56-125.163.amzn2023.x86_64', False),
            ('amzn', '2023', '6.18.48-109.150.amzn2023.x86_64', False),
            ('amzn', '2023', '6.1.189-236.400.amzn2023.x86_64', True),
            ('amzn', '2023', '6.1.189-236.399.amzn2023.x86_64', False),
            ('amzn', '2023', '6.12.100-123.456.amzn2023.x86_64', False),
            ('amzn', '2023', '6.18.56-125.164.amzn2023.aarch64', False),
            ('ubuntu', '2023', '6.18.56-125.164.amzn2023.x86_64', False),
            ('amzn', '2', '6.18.56-125.164.amzn2023.x86_64', False),
        ]
        for distro, version, kernel, accepted in cases:
            with self.subTest(distro=distro, version=version, kernel=kernel):
                test = gate.replace('. /etc/os-release', f'ID={distro}\nVERSION_ID={version}')
                test = test.replace('running=$(uname -r)', f'running={kernel}')
                result = subprocess.run(['bash', '-c', test], capture_output=True)
                self.assertEqual(result.returncode == 0, accepted, result.stderr.decode())

    def test_syntax_and_persistent_fail_closed_boot_order(self):
        for script in (self.script, self.heredoc('/opt/jcode-alpha/check-kernel.sh'),
                       self.heredoc('/opt/jcode-alpha/network-guard.sh')):
            subprocess.run(['bash', '-n'], input=script.encode(), check=True)
        self.assertIn('Before=sshd.service user@1000.service', self.script)
        self.assertIn('Requires=jcode-alpha-network-guard.service', self.script)
        self.assertIn('After=jcode-alpha-network-guard.service', self.script)
        self.assertIn('systemctl enable --now jcode-alpha-network-guard.service', self.script)
        guard = self.heredoc('/opt/jcode-alpha/network-guard.sh')
        self.assertIn('/opt/jcode-alpha/check-kernel.sh', guard)
        self.assertIn('sysctl -w net.mptcp.enabled=0', guard)
        self.assertIn('nft --check --file "$rules"', guard)
        self.assertIn('nft --file "$rules"', guard)
        self.assertNotIn('flush ruleset', guard)
        self.assertNotIn('systemctl enable nftables', self.script)
        self.assertIn('ip protocol tcp ip hdrlength > 5 counter drop', self.script)


if __name__ == '__main__':
    unittest.main()
