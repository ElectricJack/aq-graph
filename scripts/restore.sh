#!/usr/bin/env bash
# Restore to a NEW isolated PostgreSQL database, never over the authoritative database.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -ne 2 ]]; then printf 'Usage: %s BACKUP.dump NEW_DATABASE\n' "$0" >&2; exit 2; fi
backup=$1
restore_database=$2
if [[ ! "$restore_database" =~ ^aq_graph_restore_[a-zA-Z0-9_]+$ ]]; then
  printf '%s\n' 'Destination must start aq_graph_restore_ and use only letters, numbers and underscores.' >&2; exit 2
fi
test -s "$backup"
expected_checksum=$(awk 'NR==1 { print $1 }' "${backup}.sha256")
actual_checksum=$(sha256sum -- "$backup" | awk '{ print $1 }')
if [[ "$expected_checksum" != "$actual_checksum" ]]; then
  printf '%s\n' 'Backup checksum mismatch; restore refused.' >&2; exit 2
fi
docker compose exec -T postgres createdb --username=aq_graph --owner=aq_graph "$restore_database"
docker compose exec -T postgres pg_restore --username=aq_graph --dbname="$restore_database" \
  --exit-on-error --single-transaction --no-owner --no-acl < "$backup"
docker compose exec -T postgres psql --username=aq_graph --dbname="$restore_database" \
  --no-psqlrc --set=ON_ERROR_STOP=1 --command='ANALYZE;'
printf 'Restored isolated database: %s\nRun the verification procedure in docs/operations.md before any cutover.\n' "$restore_database"
