#!/usr/bin/env bash
# Serveur sshd de test pour l'interop HarakaTS (conteneur de dev / CI).
# À lancer en root DANS le conteneur (jamais sur l'hôte).
# Démarre sshd sur 127.0.0.1:2222 avec un utilisateur et une clés de test.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
RUN="$DIR/run"
mkdir -p "$RUN" /run/sshd

[ -f "$RUN/host_ed25519" ] || ssh-keygen -t ed25519 -f "$RUN/host_ed25519" -N "" -q
[ -f "$RUN/client_ed25519" ] || ssh-keygen -t ed25519 -f "$RUN/client_ed25519" -N "" -q
# Lancé via sudo, la clé client appartiendrait à root : le test (utilisateur normal) doit pouvoir la lire.
if [ -n "${SUDO_USER:-}" ]; then chown "$SUDO_USER" "$RUN/client_ed25519" "$RUN/client_ed25519.pub"; fi

id harakats-test >/dev/null 2>&1 || useradd -m -s /bin/bash harakats-test
echo "harakats-test:interop-pw" | chpasswd
mkdir -p /home/harakats-test/.ssh
cp "$RUN/client_ed25519.pub" /home/harakats-test/.ssh/authorized_keys
chown -R harakats-test:harakats-test /home/harakats-test/.ssh
chmod 700 /home/harakats-test/.ssh
chmod 600 /home/harakats-test/.ssh/authorized_keys

cat > "$RUN/sshd_config" <<EOF
Port 2222
ListenAddress 127.0.0.1
HostKey $RUN/host_ed25519
PidFile $RUN/sshd.pid
PasswordAuthentication yes
PubkeyAuthentication yes
KbdInteractiveAuthentication yes
UsePAM yes
PermitRootLogin no
AllowUsers harakats-test
Subsystem sftp /usr/lib/openssh/sftp-server
EOF

if [ -f "$RUN/sshd.pid" ] && kill -0 "$(cat "$RUN/sshd.pid")" 2>/dev/null; then
  echo "sshd de test déjà actif (port 2222)"
else
  /usr/sbin/sshd -f "$RUN/sshd_config" -E "$RUN/sshd.log"
  echo "sshd de test démarré (port 2222)"
fi

echo "Variables pour cargo test :"
echo "  HARAKA_SSHD_TEST=1"
echo "  HARAKA_TEST_KEY=$RUN/client_ed25519"
echo "  HARAKA_TEST_USER=harakats-test"
echo "  HARAKA_TEST_PORT=2222"
