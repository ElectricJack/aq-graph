#!/usr/bin/env bash
# Online custom-format PostgreSQL backup, with a checksum and a restrictive umask.
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
if [[ $# -ne 1 ]]; then printf 'Usage: %s OUTPUT.dump\n' "$0" >&2; exit 2; fi
output=$1
if [[ -e "$output" ]]; then printf 'Refusing to overwrite %s\n' "$output" >&2; exit 2; fi
mkdir -p "$(dirname "$output")"
temporary=$(mktemp "${output}.partial.XXXXXX")
trap 'rm -f "$temporary"' EXIT
docker compose exec -T postgres pg_dump --username=aq_graph --dbname=aq_graph --format=custom --no-owner --no-acl > "$temporary"
test -s "$temporary"
ln "$temporary" "$output"
rm "$temporary"
sha256sum "$output" > "${output}.sha256"
printf 'Backup written: %s\n' "$output"
