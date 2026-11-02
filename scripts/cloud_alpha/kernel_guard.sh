# Runs as root before provisioning SSH access or publishing readiness.
# Require a patched running kernel, never an installed-but-unbooted package.
cat > /opt/jcode-alpha/check-kernel.sh <<'EOF'
#!/bin/bash
set -euo pipefail
. /etc/os-release
[ "$ID" = amzn ]
[ "$VERSION_ID" = 2023 ]
running=$(uname -r)
[[ "$running" =~ ^6[.](1|18)[.][0-9]+-[0-9]+[.][0-9]+[.]amzn2023[.]x86_64$ ]]
case "$running" in
  6.1.*) floor=6.1.189-236.400.amzn2023.x86_64 ;;
  6.18.*) floor=6.18.56-125.164.amzn2023.x86_64 ;;
  *) exit 1 ;;
esac
[ "$(printf '%s\n' "$floor" "$running" | sort -V | head -n1)" = "$floor" ]
EOF
chmod 755 /opt/jcode-alpha/check-kernel.sh
/opt/jcode-alpha/check-kernel.sh

install -d -m 755 /etc/jcode-alpha
cat > /etc/jcode-alpha/cve-2026-98432.nft <<'EOF'
table inet cve_2026_98432 {
    chain input {
        type filter hook input priority -310; policy accept;
        ip protocol tcp ip hdrlength > 5 counter drop
    }
}
EOF
cat > /etc/sysctl.d/90-jcode-alpha-mptcp.conf <<'EOF'
-net.mptcp.enabled = 0
EOF
cat > /opt/jcode-alpha/network-guard.sh <<'EOF'
#!/bin/bash
set -euo pipefail
/opt/jcode-alpha/check-kernel.sh
if [ -e /proc/sys/net/mptcp/enabled ]; then
  sysctl -w net.mptcp.enabled=0
  [ "$(cat /proc/sys/net/mptcp/enabled)" = 0 ]
fi
rules=$(mktemp)
trap 'rm -f "$rules"' EXIT
if nft list table inet cve_2026_98432 >/dev/null 2>&1; then
  echo 'delete table inet cve_2026_98432' > "$rules"
fi
cat /etc/jcode-alpha/cve-2026-98432.nft >> "$rules"
nft --check --file "$rules"
nft --file "$rules"
EOF
chmod 755 /opt/jcode-alpha/network-guard.sh
cat > /etc/systemd/system/jcode-alpha-network-guard.service <<'EOF'
[Unit]
Description=Jcode alpha patched kernel and network safeguards
Before=sshd.service user@1000.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/opt/jcode-alpha/network-guard.sh
[Install]
WantedBy=multi-user.target
EOF
for unit in sshd.service user@1000.service; do
  install -d -m 755 "/etc/systemd/system/$unit.d"
  cat > "/etc/systemd/system/$unit.d/10-jcode-alpha-guard.conf" <<'EOF'
[Unit]
Requires=jcode-alpha-network-guard.service
After=jcode-alpha-network-guard.service
EOF
done
dnf install -y nftables
systemctl daemon-reload
systemctl enable --now jcode-alpha-network-guard.service
