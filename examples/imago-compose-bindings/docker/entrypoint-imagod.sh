#!/bin/sh
set -eu

mkdir -p /root/.ssh /run/sshd
for _ in $(seq 1 100); do
    if [ -f /ssh-control/id_ed25519.pub ]; then
        break
    fi
    sleep 0.1
done
if [ ! -f /ssh-control/id_ed25519.pub ]; then
    echo "missing /ssh-control/id_ed25519.pub; start the imago-deployer service to initialize shared SSH credentials" >&2
    exit 1
fi
cp /ssh-control/id_ed25519.pub /root/.ssh/authorized_keys
chmod 700 /root/.ssh
chmod 600 /root/.ssh/authorized_keys

ssh-keygen -A
/usr/sbin/sshd
exec /usr/local/bin/imagod "$@"
