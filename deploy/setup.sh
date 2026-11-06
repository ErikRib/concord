#!/usr/bin/env bash
# Instala ou atualiza o Concord num servidor Ubuntu (22.04/24.04, ARM ou x86).
# Configura: Concord (systemd), LiveKit (voz), Caddy (HTTPS automático),
# firewall, backup diário e atualizações automáticas de segurança.
#
# Uso, na pasta com o código do Concord, como usuário com sudo:
#   sudo bash deploy/setup.sh SITE VOZ EMAIL
#   ex.: sudo bash deploy/setup.sh meuconcord.duckdns.org voz-meuconcord.duckdns.org eu@exemplo.com
#
# Rodar de novo é seguro: atualiza o código e mantém dados, chaves e configurações.
set -euo pipefail

SITE="${1:?informe o domínio do site (ex.: meuconcord.duckdns.org)}"
VOZ="${2:?informe o domínio da voz (ex.: voz-meuconcord.duckdns.org)}"
EMAIL="${3:?informe um e-mail para os avisos do certificado HTTPS}"
LIVEKIT_VERSION="1.13.11"
SRC="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_USER="${SUDO_USER:-root}"

[ "$(id -u)" -eq 0 ] || { echo "Rode com sudo."; exit 1; }
[ -f "$SRC/Cargo.toml" ] || { echo "Rode a partir da pasta do código do Concord."; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

step "Pacotes do sistema"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q build-essential pkg-config curl ca-certificates gnupg debian-keyring \
  debian-archive-keyring apt-transport-https iptables-persistent unattended-upgrades

step "Memória virtual (swap) para compilar em máquinas pequenas"
if ! swapon --show | grep -q /swapfile; then
  fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  grep -q /swapfile /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

step "Caddy (HTTPS automático com Let's Encrypt)"
if ! command -v caddy >/dev/null; then
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
  apt-get update -q && apt-get install -y -q caddy
fi

step "LiveKit $LIVEKIT_VERSION (servidor de mídia), com conferência do SHA-256"
case "$(uname -m)" in
  aarch64 | arm64) ARCH=arm64 ;;
  x86_64) ARCH=amd64 ;;
  *) echo "Arquitetura não suportada: $(uname -m)"; exit 1 ;;
esac
if ! /usr/local/bin/livekit-server --version 2>/dev/null | grep -q "$LIVEKIT_VERSION"; then
  TMP="$(mktemp -d)"
  BASE="https://github.com/livekit/livekit/releases/download/v$LIVEKIT_VERSION"
  FILE="livekit_${LIVEKIT_VERSION}_linux_${ARCH}.tar.gz"
  curl -fsSL -o "$TMP/$FILE" "$BASE/$FILE"
  curl -fsSL -o "$TMP/checksums.txt" "$BASE/checksums.txt"
  (cd "$TMP" && grep " $FILE\$" checksums.txt | sha256sum -c -)
  tar -xzf "$TMP/$FILE" -C "$TMP" livekit-server
  install -m 755 "$TMP/livekit-server" /usr/local/bin/livekit-server
  rm -rf "$TMP"
fi

step "Usuários de serviço e pastas"
id concord >/dev/null 2>&1 || useradd --system --home /var/lib/concord --shell /usr/sbin/nologin concord
id livekit >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin livekit
install -d -o concord -g concord -m 700 /var/lib/concord /var/backups/concord
install -d -m 755 /etc/concord

step "Chaves e configuração (geradas uma vez; nunca saem do servidor)"
if [ ! -f /etc/concord/livekit.env ]; then
  umask 077
  printf 'LK_KEY=concord%s\nLK_SECRET=%s\n' "$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')" \
    "$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')" > /etc/concord/livekit.env
fi
# shellcheck disable=SC1091
. /etc/concord/livekit.env

umask 077
cat > /etc/concord/livekit.yaml <<EOF
port: 7880
rtc:
  tcp_port: 7881
  port_range_start: 50000
  port_range_end: 60000
  use_external_ip: true
turn:
  enabled: true
  domain: $VOZ
  udp_port: 3478
keys:
  $LK_KEY: $LK_SECRET
room:
  empty_timeout: 30
logging:
  level: warn
EOF
chown livekit /etc/concord/livekit.yaml
chmod 600 /etc/concord/livekit.yaml

if [ ! -f /etc/concord/concord.env ]; then
  cat > /etc/concord/concord.env <<EOF
# Configuração do Concord. Depois de editar: sudo systemctl restart concord
PORT=3000
CONCORD_HOST=127.0.0.1
CONCORD_DB=/var/lib/concord/concord.db
CONCORD_DATA=/var/lib/concord/concord-data.json
CONCORD_RETENCAO_DIAS=180
# LGPD: identifique quem opera esta instância e o canal de atendimento ao titular.
CONCORD_CONTROLADOR=
CONCORD_CONTATO=
# Seu id em "Baixar meus dados" -> id_pseudonimo (vários: separados por vírgula).
CONCORD_ADMINS=
EOF
fi
# Endereços e chaves do LiveKit são sempre regravados (seguem os domínios informados).
sed -i '/^CONCORD_LIVEKIT_/d' /etc/concord/concord.env
cat >> /etc/concord/concord.env <<EOF
CONCORD_LIVEKIT_URL=wss://$VOZ
CONCORD_LIVEKIT_API_URL=http://127.0.0.1:7880
CONCORD_LIVEKIT_KEY=$LK_KEY
CONCORD_LIVEKIT_SECRET=$LK_SECRET
EOF
chown root:concord /etc/concord/concord.env
chmod 640 /etc/concord/concord.env

step "Compilando o Concord (alguns minutos na primeira vez)"
BUILD_HOME="$(getent passwd "$BUILD_USER" | cut -d: -f6)"
if ! sudo -u "$BUILD_USER" bash -lc 'command -v cargo' >/dev/null 2>&1 && [ ! -x "$BUILD_HOME/.cargo/bin/cargo" ]; then
  sudo -u "$BUILD_USER" bash -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'
fi
sudo -u "$BUILD_USER" bash -lc "cd '$SRC' && \$HOME/.cargo/bin/cargo build --release"
systemctl stop concord 2>/dev/null || true
install -m 755 "$SRC/target/release/concord" /usr/local/bin/concord

step "Serviços (reiniciam sozinhos e sobem com a máquina)"
cat > /etc/systemd/system/livekit.service <<'EOF'
[Unit]
Description=LiveKit (voz do Concord)
After=network-online.target
Wants=network-online.target

[Service]
User=livekit
ExecStart=/usr/local/bin/livekit-server --config /etc/concord/livekit.yaml
Restart=always
RestartSec=3
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

cat > /etc/systemd/system/concord.service <<'EOF'
[Unit]
Description=Concord
After=network-online.target livekit.service
Wants=network-online.target

[Service]
User=concord
EnvironmentFile=/etc/concord/concord.env
WorkingDirectory=/var/lib/concord
ExecStart=/usr/local/bin/concord
Restart=always
RestartSec=3
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/concord

[Install]
WantedBy=multi-user.target
EOF

step "Caddy: HTTPS do site e da voz (sem log de acesso: nenhum IP é registrado)"
cat > /etc/caddy/Caddyfile <<EOF
{
	email $EMAIL
}

$SITE {
	encode zstd gzip
	reverse_proxy 127.0.0.1:3000
}

$VOZ {
	reverse_proxy 127.0.0.1:7880
}
EOF

step "Backup diário (guarda 7 dias, dentro do prazo de retenção)"
cat > /etc/cron.daily/concord-backup <<'EOF'
#!/bin/sh
set -e
set -a; . /etc/concord/concord.env; set +a
DEST=/var/backups/concord/concord-$(date +%F).db
rm -f "$DEST"
runuser -u concord -- env CONCORD_DB="$CONCORD_DB" /usr/local/bin/concord backup "$DEST"
find /var/backups/concord -name 'concord-*.db' -mtime +7 -delete
EOF
chmod 755 /etc/cron.daily/concord-backup

step "Firewall da máquina (o painel da Oracle também precisa liberar as mesmas portas)"
open_port() { # protocolo porta
  iptables -C INPUT -p "$1" -m state --state NEW --dport "$2" -j ACCEPT 2>/dev/null ||
    iptables -I INPUT -p "$1" -m state --state NEW --dport "$2" -j ACCEPT
}
open_port tcp 80
open_port tcp 443
open_port tcp 7881
open_port udp 3478
open_port udp 50000:60000
netfilter-persistent save >/dev/null

step "Atualizações automáticas de segurança"
dpkg-reconfigure -f noninteractive unattended-upgrades

systemctl daemon-reload
systemctl enable --now livekit concord caddy
systemctl restart livekit concord caddy
sleep 3

step "Pronto"
systemctl --no-pager --lines=0 status livekit concord caddy | grep -E '●|Active' || true
cat <<EOF

Acesse: https://$SITE   (o primeiro acesso pode levar 1 minuto, enquanto o HTTPS é emitido)

Próximos passos:
  1. Edite /etc/concord/concord.env (CONCORD_CONTROLADOR, CONCORD_CONTATO e CONCORD_ADMINS)
     e rode: sudo systemctl restart concord
  2. Logs: sudo journalctl -u concord -u livekit -f
EOF
