#!/usr/bin/env bash
# Reset one database to empty by dropping and recreating it inside its already-running
# container — the cheap alternative to tearing the container down and cold-starting a fresh
# one. Used between the live suites (which leave their last schema/ledger behind) and the
# example scenarios (which each expect an empty database).
#
# Usage: ci/db-reset.sh <maria|pg> <container> <db> <password>
set -euo pipefail

engine="${1:?usage: db-reset.sh <maria|pg> <container> <db> <password>}"
container="${2:?container name}"
db="${3:?database name}"
pass="${4:?password}"

command -v docker >/dev/null 2>&1 || { echo "  db-reset: docker not found — skip $container"; exit 0; }
if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || echo no)" != "true" ]]; then
  echo "  db-reset: $container not running — skip"
  exit 0
fi

echo "  reset    $db on $container (drop+create)"
case "$engine" in
  maria)
    docker exec -i "$container" mariadb -uroot -p"$pass" \
      -e "DROP DATABASE IF EXISTS \`$db\`; CREATE DATABASE \`$db\`;" ;;
  pg)
    # FORCE (Postgres 13+) drops even with lingering connections from the prior suite.
    docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS \"$db\" WITH (FORCE);" \
      -c "CREATE DATABASE \"$db\";" >/dev/null ;;
  *) echo "db-reset: unknown engine '$engine'" >&2; exit 1 ;;
esac
echo "  reset    $container done"
