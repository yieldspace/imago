#!/bin/sh
set -eu

mkdir -p /root/.ssh /run/sshd
cp /etc/imago/example/ssh/control/id_ed25519.pub /root/.ssh/authorized_keys
chmod 700 /root/.ssh
chmod 600 /root/.ssh/authorized_keys

/usr/sbin/sshd
exec /usr/local/bin/imagod "$@"
