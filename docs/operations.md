# Operating aq-graph

This runbook applies to the standalone Compose stack and one authoritative PostgreSQL database. It never requires an Agent Q installation or access to AQ data. Local setup binds HTTP to loopback; PostgreSQL remains on an internal network without a host port. Remote clients need an authenticated HTTPS/private-network route, configured separately from the graph domain.

## Configuration and readiness

`DATABASE_URL` is required by `serve`. The default bind is `127.0.0.1:8741`; the Compose container binds internally to `0.0.0.0:8741`, with a loopback-only host mapping. `AQ_GRAPH_POOL_SIZE` defaults to 20 and is limited to 1–128. Startup retries connection establishment ten times, applies embedded migrations, and fails before serving if migration or bootstrap fails. PostgreSQL driver errors, credentials, connection URLs and request bodies are deliberately excluded from application error output.

`GET /health/live` checks process reachability. `GET /health/ready` checks database/schema readiness. Neither requires credentials. Compose waits for PostgreSQL health before starting the service and probes service readiness. A successful readiness response does not prove spare capacity. Monitor error rates, overload responses, lock waits, pool waits and resource use during traffic.

The service admits at most 256 application requests and holds admission through response buffering and serialization; it releases admission before socket-write completion. Axum does not cap accepted connection count. A production ingress must bound connections, header/body read time, response write time and slow clients. The service bounds request/response bodies and application request duration. Clients must use server caps described in [docs/api.md](api.md), consume pagination and respect context truncation. HTTP 503/429 includes retry guidance; use jittered backoff and preserve mutation idempotency keys. A timeout may occur after a transaction committed, so a fresh key is unsafe for retry.

## Credentials

`init` writes a mode-0600 JSON config containing an endpoint, workspace UUID and credential file path. It does not configure or clone a server. Client overrides are `AQ_GRAPH_CONFIG`, `AQ_GRAPH_ENDPOINT`, `AQ_GRAPH_WORKSPACE`, `AQ_GRAPH_TOKEN_FILE`, or `AQ_GRAPH_TOKEN`. A configured file takes precedence over the token environment variable. `--token-file` is the only credential-related CLI argument; never pass token values through arguments.

Initial service bootstrap accepts `AQ_GRAPH_BOOTSTRAP_WORKSPACE`, `AQ_GRAPH_BOOTSTRAP_ACTOR`, and `AQ_GRAPH_BOOTSTRAP_TOKEN_FILE` or `AQ_GRAPH_BOOTSTRAP_TOKEN`. Only token hashes persist. Bootstrap may initialize a new workspace or verify the matching existing admin credential; it cannot replace workspace credentials. Removing the original credential while retaining stale bootstrap configuration will prevent restart.

After the first successful bootstrap, remove all bootstrap environment settings from the service configuration before credential rotation. For Compose, edit the service environment block to remove `AQ_GRAPH_BOOTSTRAP_WORKSPACE`, `AQ_GRAPH_BOOTSTRAP_ACTOR`, `AQ_GRAPH_BOOTSTRAP_TOKEN_FILE`, and remove the bootstrap-token service mount. Keep your administrative client token in its private file. Restart and verify authenticated reads before rotating. Store recovery credentials securely off-host.

Admins can grant/rotate scoped identities with `POST /v1/workspaces/WORKSPACE/credentials` and revoke them with `DELETE /v1/workspaces/WORKSPACE/credentials/ACTOR`. Use the versioned API with credentials sourced securely in memory, not shell arguments or checked-in JSON. Read, write and admin are separate capabilities, scoped to a workspace. Ordinary graph writers cannot erase history or omit required historical writes. Keep an independently tested admin credential available when rotating; do not revoke your only administrative route.

The local initializer creates `.local` mode 0700, client token mode 0600, and PostgreSQL password mode 0644 under that private directory. The service runs with the local user's uid/gid; only the selected secret files are bind-mounted. PostgreSQL's separate unprivileged uid needs the password-file read permission. Protect the parent directory and backups. The production image itself defaults to uid/gid 10001 and does not need a writable filesystem.

## Backup

Backups contain durable graph content, history and credential hashes. Treat them as sensitive data. Keep durable history indefinitely during the pilot. Do not prune receipt/history tables without a separately documented retention and pinned-revision policy. Diagnostics have bounded Docker log retention (three files, ten megabytes each).

```sh
scripts/backup.sh /secure/backups/aq-graph-2026-10-29.dump
```

The helper runs online `pg_dump` in custom format, with no owner/ACL statements, under `umask 077`; it writes a temporary file, checks nonempty output, then publishes a SHA-256 checksum. It refuses to overwrite an existing destination. PostgreSQL provides a consistent snapshot while the service remains live. The database must retain production-equivalent durability settings. The helper backs up the application database only, not cluster roles, external artifacts, secrets or TLS configuration. Archive those separately with the deployment manifest and binary/image version.

Copy both dump and checksum to encrypted off-host storage. Set a backup interval and retention policy that match your recovery point objective. A checksum proves transport integrity, not database recoverability. Schedule restoration drills and retain their evidence.

## Restore and verification

Restore first into a new isolated database; the helper refuses an authoritative destination name:

```sh
scripts/restore.sh /secure/backups/aq-graph-2026-10-29.dump aq_graph_restore_drill_20261029
```

It verifies the checksum, creates a fresh database, restores with a single transaction and error-stop, then analyzes tables. Existing destination databases cause a safe failure. A failed restore may leave an empty destination; inspect and drop only that isolated destination before retrying.

Before cutover:

1. Record source counts for nodes, edges, revisions, history, change sets and receipts at backup time, using a consistent read-only database snapshot. Compare restored counts and representative immutable revision hashes. New source writes after backup naturally change current source counts.
2. Start the **same image version** on an isolated loopback port against the restored database, with bootstrap disabled. Verify readiness and authentication. Keep it unreachable from production consumers.
3. Exercise representative get/context, ready, search, paginated history, diff, pinned revisions, draft inspection and idempotent replay using read-only requests wherever possible. Execute write checks only on the isolated clone. Check both authorized access and rejected workspace/credential access.
4. Restart the isolated service and recheck committed data and idempotency. Document backup timestamp, counts, hashes, binary/schema versions, verification commands, results and recovery time.
5. For actual recovery, stop original writers and isolate the old service, explicitly select the restored database, start the compatible service, verify again, then route clients to it. Retain the old database until reconciliation and recovery approval are complete.

The scripts never change the live service connection string or route clients. Restoring a prior backup loses later writes unless a separately configured WAL/PITR recovery process covers them. Off-host backups and WAL archiving are operator responsibilities. PostgreSQL major upgrades require a separate tested procedure.

## Upgrade, rollback and shutdown

Pin released images by version, and preferably digest. Back up and verify recovery before migrating. Record the current schema version from `_sqlx_migrations`, release identifier and relevant configuration. Run the proposed image against an isolated restored copy and verify compatibility, migration timing and representative reads/writes before live rollout.

For this initial release API version is 1 and schema version is migration `0005`. Do not start an old image against a newly migrated incompatible schema. Rollback may require restoring a backup and reconciling later writes; image replacement alone is insufficient. Future releases must explicitly state compatible schema ranges and irreversible changes.

Migrations `0004` and `0005` tighten the standalone baseline to selected v1 contracts. They refuse incompatible existing endpoint kinds, multiple structural parents, excessive depth, combined dependency cycles, or verified/retired knowledge without the new attribution/reason fields. They never delete invalid edges or invent verification evidence. Restore a backup into an isolated database, inspect the migration diagnostics, and reconcile the affected records explicitly before retrying. Historical snapshots stay unchanged; current authors are recovered from the original revision. Older arbitrary provenance may remain readable but must be reconciled to typed descriptors before editing or restoring that content. Migration preflight is not a v1 data importer.

`docker compose stop graph` sends termination and allows 40 seconds for request draining. The HTTP server handles SIGINT/SIGTERM gracefully. Keep clients' original idempotency keys for interrupted/uncertain requests, restart, check readiness, and retry identical operations. Never delete the PostgreSQL volume during restart or upgrade. `docker compose down` preserves it; `down --volumes` is destructive and is reserved for disposable test stacks.

## Diagnostics

Inspect `docker compose ps` and bounded `docker compose logs --tail 100 graph`. Avoid debug logging of HTTP headers, request bodies, environment variables or connection strings. For PostgreSQL, inspect `pg_stat_activity`, `pg_stat_database`, `pg_locks`, and query plans using trusted administrative access. Investigate long transactions, pool exhaustion and serialization conflicts before increasing capacity. No unbounded request/transaction retry loops are permitted.

The load harness in [benchmark.md](benchmark.md) reports client-visible failures and latency, including overload and expected semantic rejects. Host CPU/memory, database lock/I/O behavior and service-observed request timing need separate collection. A green readiness endpoint or a successful-request p95 alone does not establish production capacity.
