#!/usr/bin/env bash
# Verify certificate-checked CLI and embedded connections on disposable server fixtures.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
fixture_dir=$(mktemp -d /tmp/based-tls.XXXXXX)
pg_container="based-tls-pg-$$"
maria_container="based-tls-maria-$$"
cleanup() {
  docker rm -fv "$pg_container" "$maria_container" >/dev/null 2>&1 || true
  rm -rf "$fixture_dir"
}
trap cleanup EXIT
bash ci/tls/certificates.sh "$fixture_dir/certs"
mkdir -p "$fixture_dir/init"
cat > "$fixture_dir/init/require-tls.sh" <<'INIT'
#!/usr/bin/env bash
sed -i '1i hostnossl all all all reject' "$PGDATA/pg_hba.conf"
INIT
# Postgres requires its leaf key to be owned by postgres and mode 0600.
docker run -d --name "$pg_container" -p 127.0.0.1::5432 \
  -e POSTGRES_PASSWORD=based_tls_test_pw -e POSTGRES_DB=based_test \
  -v "$fixture_dir/certs:/certs:ro" -v "$fixture_dir/init:/docker-entrypoint-initdb.d:ro" \
  postgres:16 bash -c 'cp /certs/server.key /tmp/server.key; chown postgres /tmp/server.key; chmod 600 /tmp/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/certs/server.crt -c ssl_key_file=/tmp/server.key' >/dev/null
docker run -d --name "$maria_container" -p 127.0.0.1::3306 \
  -e MARIADB_ROOT_PASSWORD=based_tls_test_pw -e MARIADB_DATABASE=based_test \
  -v "$fixture_dir/certs:/certs:ro" mariadb:11.4 \
  --ssl-ca=/certs/ca.crt --ssl-cert=/certs/server.crt --ssl-key=/certs/server.key \
  --require-secure-transport=ON >/dev/null
wait_for_server() {
  local container="$1" kind="$2"
  for _ in $(seq 1 90); do
    if [[ "$kind" == pg ]] && docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then return; fi
    if [[ "$kind" == maria ]] && docker exec "$container" healthcheck.sh --connect --innodb_initialized >/dev/null 2>&1; then return; fi
    sleep 1
  done
  docker logs "$container" >&2
  return 1
}
wait_for_server "$pg_container" pg
wait_for_server "$maria_container" maria
pg_port=$(docker port "$pg_container" 5432/tcp | cut -d: -f2)
maria_port=$(docker port "$maria_container" 3306/tcp | cut -d: -f2)
export TEST_TLS_POSTGRES_URL="postgres://postgres:based_tls_test_pw@localhost:$pg_port/based_test?sslmode=verify-full&sslrootcert=$fixture_dir/certs/ca.crt"
export TEST_TLS_MARIADB_URL="mysql://root:based_tls_test_pw@localhost:$maria_port/based_test?ssl-mode=VERIFY_IDENTITY&ssl-ca=$fixture_dir/certs/ca.crt"
cargo test -p based-runtime --features mariadb,postgres,tls-rustls --test database_tls -- --nocapture
cargo build -p based-cli
# Each copied quickstart proves CLI migration and a generated embedded typed call.
for kind in postgres mariadb; do
  example="$fixture_dir/$kind"
  mkdir -p "$example"
  for entry in Cargo.toml Cargo.lock based.toml schema migrations src generated; do
    cp -R "examples/$kind-quickstart/$entry" "$example/"
  done
  # Preserve the standalone consumer's path dependencies while isolating all data/artifacts.
  python3 - "$example/Cargo.toml" "$root" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
p.write_text(p.read_text().replace('../../crates/', sys.argv[2] + '/crates/'))
PY
  url="$TEST_TLS_POSTGRES_URL"
  if [[ "$kind" == mariadb ]]; then url="$TEST_TLS_MARIADB_URL"; fi
  "$root/target/debug/based" migrate apply "$example" --database-url "$url"
  DATABASE_URL="$url" CARGO_TARGET_DIR="$root/examples/$kind-quickstart/target" cargo run --manifest-path "$example/Cargo.toml" --features tls
  # Both malformed trust configurations must fail in the installed CLI too.
  for bad_url in "${url/@localhost:/@127.0.0.1:}" "${url/ca.crt/untrusted.crt}"; do
    if "$root/target/debug/based" migrate status "$example" --database-url "$bad_url" >"$fixture_dir/out" 2>"$fixture_dir/err"; then
      echo 'CLI accepted an invalid TLS certificate/hostname' >&2; exit 1
    fi
    if rg -q 'based_tls_test_pw' "$fixture_dir/out" "$fixture_dir/err"; then
      echo 'CLI exposed a database credential' >&2; exit 1
    fi
  done
done
# Run the existing live contracts with the verified connection settings.
make ci-live-mariadb MARIADB_URL="$TEST_TLS_MARIADB_URL"
make ci-live-postgres POSTGRES_URL="$TEST_TLS_POSTGRES_URL"
echo 'ci-database-tls: verified CLI/embedded connections and live suites; invalid trust rejected'
