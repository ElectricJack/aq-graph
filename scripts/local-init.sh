#!/usr/bin/env bash
# Generate local-only credentials and an independent CLI configuration.
set -euo pipefail
cd "$(dirname "$0")/.."
umask 077
mkdir -p .local
python3 - <<'PY'
import json, os, pathlib, secrets
root = pathlib.Path('.local').resolve()
for name in ('token', 'postgres-password'):
    path = root / name
    if not path.exists():
        with path.open('x') as f:
            f.write(secrets.token_hex(32))
    path.chmod(0o600)
config = root / 'config.json'
if not config.exists():
    with config.open('x') as f:
        json.dump({'endpoint':'http://127.0.0.1:' + os.environ.get('AQ_GRAPH_PORT', '8741'),'workspace':'00000000-0000-4000-8000-000000000001','token_file':str(root / 'token')}, f, indent=2)
config.chmod(0o600)
PY
# Docker bind-mounted secrets preserve host ownership. Match the service uid to
# the local CLI owner, while keeping the service unprivileged.
chmod 700 .local
chmod 644 .local/postgres-password
if [[ ! -e .env ]]; then
  printf 'AQ_GRAPH_UID=%s\nAQ_GRAPH_GID=%s\n' "$(id -u)" "$(id -g)" > .env
else
  grep -q '^AQ_GRAPH_UID=' .env || printf '\nAQ_GRAPH_UID=%s\n' "$(id -u)" >> .env
  grep -q '^AQ_GRAPH_GID=' .env || printf '\nAQ_GRAPH_GID=%s\n' "$(id -g)" >> .env
fi
printf '%s\n' 'Local credentials created. Start: docker compose up --build -d' \
  'Client: export AQ_GRAPH_CONFIG="$PWD/.local/config.json"' \
  'The token and PostgreSQL port are never published by this setup.'
