#!/bin/bash
# Cloud-init runs as root once. No personal credentials are placed in user-data.
set -euo pipefail
exec > >(tee -a /var/log/jcode-alpha-bootstrap.log) 2>&1
# A failed bootstrap must not leave an unattended machine running indefinitely.
trap 'systemctl poweroff' ERR
install -d -m 755 /opt/jcode-alpha
# Install the local fail-safe BEFORE network package work.
cat > /etc/systemd/system/jcode-alpha-lease.service <<'EOF'
[Unit]
Description=Jcode alpha two-hour fail-safe lease
[Service]
Type=oneshot
ExecStart=/usr/bin/systemctl poweroff
EOF
cat > /etc/systemd/system/jcode-alpha-lease.timer <<'EOF'
[Unit]
Description=Jcode alpha maximum boot lease
[Timer]
OnBootSec=2h
AccuracySec=10s
[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now jcode-alpha-lease.timer
dnf install -y git tmux python3 tar gzip openssl nodejs npm gcc gcc-c++ make
install -d -m 700 -o ec2-user -g ec2-user /home/ec2-user/.ssh /home/ec2-user/.jcode
install -d -m 755 -o ec2-user -g ec2-user /home/ec2-user/.local/bin /home/ec2-user/workspaces
printf '%s\n' '__SSH_PUBLIC_KEY__' > /home/ec2-user/.ssh/authorized_keys
chmod 600 /home/ec2-user/.ssh/authorized_keys
chown ec2-user:ec2-user /home/ec2-user/.ssh/authorized_keys
cat > /etc/ssh/sshd_config.d/20-jcode-alpha.conf <<'EOF'
PasswordAuthentication no
PermitRootLogin no
AllowUsers ec2-user
EOF
sshd -t
systemctl reload sshd
# Pinned published Jcode release, verified against independently fetched manifest.
curl --fail --location --retry 3 --connect-timeout 10 --max-time 180 \
  'https://github.com/1jehuang/jcode/releases/download/v0.84.0/jcode-linux-x86_64.tar.gz' \
  -o /opt/jcode-alpha/jcode.tar.gz
printf '%s  %s\n' 'e00eaede1a4f26812e77382bda9d82e2d301affd983d18ada38fddd43dce9571' /opt/jcode-alpha/jcode.tar.gz | sha256sum --check -
tar xzf /opt/jcode-alpha/jcode.tar.gz -C /opt/jcode-alpha
mv /opt/jcode-alpha/jcode-linux-x86_64 /opt/jcode-alpha/jcode
chmod 755 /opt/jcode-alpha/jcode
# Wrapper gives both noninteractive SDK SSH connections and terminals the same environment.
cat > /home/ec2-user/.local/bin/jcode <<'EOF'
#!/bin/sh
export AWS_REGION=us-east-1
export AWS_DEFAULT_REGION=us-east-1
export JCODE_BEDROCK_ENABLE=1
export JCODE_BEDROCK_MODEL=amazon.nova-micro-v1:0
export JCODE_SERVER_NAME=jcode-cloud-alpha
export JCODE_NO_UPDATE=1
exec /opt/jcode-alpha/jcode "$@"
EOF
chmod 755 /home/ec2-user/.local/bin/jcode
chown ec2-user:ec2-user /home/ec2-user/.local/bin/jcode
cat > /home/ec2-user/.jcode/config.toml <<'EOF'
[provider]
default_provider = "bedrock"
default_model = "amazon.nova-micro-v1:0"
EOF
chown ec2-user:ec2-user /home/ec2-user/.jcode/config.toml
chmod 600 /home/ec2-user/.jcode/config.toml
cat > /home/ec2-user/workspaces/README.md <<'EOF'
# Jcode Cloud personal alpha

This encrypted workspace survives stop/start. Clone projects here.
Multiple Desktop panels are sessions on this same machine.
The initial test model is Amazon Nova Micro through a narrowly scoped instance role.
No laptop credentials or repositories have been copied here.

The alpha has a TWO-HOUR maximum continuous runtime lease and 50 machine-hours
per UTC month. The lease stops the VM even during work. Files survive but running
processes do not. The public subscription product is not launched or billed yet.
Use the local jcode-cloud-alpha helper to check usage, wake, or stop this machine.
EOF
chown ec2-user:ec2-user /home/ec2-user/workspaces/README.md
runuser -u ec2-user -- /home/ec2-user/.local/bin/jcode --version
systemctl enable --now amazon-ssm-agent
touch /opt/jcode-alpha/ready
trap - ERR
