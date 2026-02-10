#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CERTS_DIR="${ROOT_DIR}/certs"

mkdir -p "${CERTS_DIR}"
rm -f "${CERTS_DIR}"/*.crt "${CERTS_DIR}"/*.key "${CERTS_DIR}"/*.csr "${CERTS_DIR}"/*.srl "${CERTS_DIR}"/*.ext

openssl genrsa -out "${CERTS_DIR}/ca.key" 4096
openssl req -x509 -new -nodes -key "${CERTS_DIR}/ca.key" -sha256 -days 3650 \
  -subj "/CN=imago-local-ca" -out "${CERTS_DIR}/ca.crt"

openssl genrsa -out "${CERTS_DIR}/server.key" 2048
openssl req -new -key "${CERTS_DIR}/server.key" -subj "/CN=localhost" \
  -out "${CERTS_DIR}/server.csr"
cat > "${CERTS_DIR}/server.ext" <<'EOF'
subjectAltName=DNS:localhost,IP:127.0.0.1
extendedKeyUsage=serverAuth
EOF
openssl x509 -req -in "${CERTS_DIR}/server.csr" -CA "${CERTS_DIR}/ca.crt" \
  -CAkey "${CERTS_DIR}/ca.key" -CAserial "${CERTS_DIR}/ca.srl" -CAcreateserial \
  -out "${CERTS_DIR}/server.crt" -days 3650 -sha256 -extfile "${CERTS_DIR}/server.ext"

openssl genrsa -out "${CERTS_DIR}/client.key" 2048
openssl req -new -key "${CERTS_DIR}/client.key" -subj "/CN=imago-local-client" \
  -out "${CERTS_DIR}/client.csr"
cat > "${CERTS_DIR}/client.ext" <<'EOF'
extendedKeyUsage=clientAuth
EOF
openssl x509 -req -in "${CERTS_DIR}/client.csr" -CA "${CERTS_DIR}/ca.crt" \
  -CAkey "${CERTS_DIR}/ca.key" -CAserial "${CERTS_DIR}/ca.srl" \
  -out "${CERTS_DIR}/client.crt" -days 3650 -sha256 -extfile "${CERTS_DIR}/client.ext"

chmod 600 "${CERTS_DIR}/ca.key" "${CERTS_DIR}/server.key" "${CERTS_DIR}/client.key"
rm -f "${CERTS_DIR}/server.csr" "${CERTS_DIR}/client.csr" "${CERTS_DIR}/server.ext" "${CERTS_DIR}/client.ext"

echo "generated certificates in ${CERTS_DIR}"
