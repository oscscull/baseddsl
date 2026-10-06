#!/usr/bin/env bash
# Disposable CA and localhost-only leaf certificate. Never use these keys in production.
set -euo pipefail
cert_dir="${1:?certificate directory}"
mkdir -p "$cert_dir"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=Based-Test-CA \
  -keyout "$cert_dir/ca.key" -out "$cert_dir/ca.crt" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj /CN=localhost \
  -keyout "$cert_dir/server.key" -out "$cert_dir/server.csr" >/dev/null 2>&1
printf 'subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n' > "$cert_dir/leaf.ext"
openssl x509 -req -days 2 -in "$cert_dir/server.csr" -CA "$cert_dir/ca.crt" \
  -CAkey "$cert_dir/ca.key" -CAcreateserial -extfile "$cert_dir/leaf.ext" \
  -out "$cert_dir/server.crt" >/dev/null 2>&1
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=Untrusted-Test-CA \
  -keyout "$cert_dir/untrusted.key" -out "$cert_dir/untrusted.crt" >/dev/null 2>&1
# Container users can read the disposable leaf key; the CA signing keys remain private.
chmod 755 "$cert_dir"
chmod 644 "$cert_dir/server.key" "$cert_dir/"*.crt
