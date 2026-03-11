#!/bin/sh
set -eu

mkdir -p /root/.ssh /ssh-control
chmod 700 /root/.ssh /ssh-control

if [ ! -f /ssh-control/id_ed25519 ] || [ ! -f /ssh-control/id_ed25519.pub ]; then
    ssh-keygen -q -t ed25519 -N '' -C imago-compose-bindings-control -f /ssh-control/id_ed25519
fi

cp /ssh-control/id_ed25519 /root/.ssh/id_ed25519
chmod 600 /root/.ssh/id_ed25519
touch /root/.ssh/known_hosts.imago-compose
chmod 600 /root/.ssh/known_hosts.imago-compose
cat > /root/.ssh/config <<'EOF'
Host imagod-alice imagod-bob
    StrictHostKeyChecking accept-new
    UserKnownHostsFile /root/.ssh/known_hosts.imago-compose
    LogLevel ERROR
EOF
chmod 600 /root/.ssh/config

exec "$@"
