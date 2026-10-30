# aq-graph

A standalone Rust service and CLI for a shared task and knowledge graph. PostgreSQL holds current records, immutable logical record revisions, lifecycle events, relationship history, atomic batches and named drafts. It runs independently of Agent Q and never reads AQ databases. AQ v2 is a fresh start. V1 import and migration support are outside this repository; any optional one-off transfer belongs in an external script. Normal aq-graph schema upgrades and backup/restore remain supported.

The baseline includes workspace-scoped read/write/admin credentials, version-checked mutations, idempotency, dependency and parent-child cycle protection, intrinsic readiness, bounded context, full-text record search, shared source/date/summary metadata, history and content restoration. A ready task is an active, nonretired leaf with every blocking prerequisite completed and no draft or retired structural ancestor; readiness is not an execution claim. `A blocks B` means B depends on A.

See [implementation status and verification limits](docs/implementation-status.md) for what is available and what remains before deployment and fresh AQ adoption.

[Local performance results](docs/performance/README.md) include the 100,000-node/500,000-edge benchmark and raw evidence.

## Start locally

Requirements: Docker Engine with Compose v2, Python 3.10+, and the CLI binary (or Rust 1.96.1 to build it). The stack exposes only authenticated HTTP on `127.0.0.1:8741`. PostgreSQL has no published port.

```sh
scripts/local-init.sh
# If building the CLI from source:
cargo build --locked --release
export PATH="$PWD/target/release:$PATH"
export AQ_GRAPH_CONFIG="$PWD/.local/config.json"
docker compose up --build -d --wait

aq-graph health
aq-graph task create --title "Implement login" --state active --type procedure
aq-graph task ready --json
```

`local-init.sh` creates independent random credentials in a private `.local` directory, preserves them on reruns, and configures the local client. It writes the host uid/gid to `.env` so the unprivileged service can read the owner-only token file. The PostgreSQL password file is readable through its individual container mount; its host parent directory is mode 0700. Do not place credentials or `.local` in source control. Back up the initial admin credential securely.

For a native PostgreSQL setup, supply `DATABASE_URL`, then run:

```sh
aq-graph --config .local/native.json --token-file .local/native-token init --local
export DATABASE_URL='postgres://USER@127.0.0.1/aq_graph'
export AQ_GRAPH_BOOTSTRAP_WORKSPACE=00000000-0000-4000-8000-000000000001
export AQ_GRAPH_BOOTSTRAP_TOKEN_FILE="$PWD/.local/native-token"
aq-graph serve
```

`serve` connects with a bounded pool, applies embedded migrations, optionally bootstraps one workspace, and handles termination gracefully. Bootstrap cannot replace credentials for an existing workspace. After initialization, remove bootstrap settings before rotating credentials; see the [operations runbook](docs/operations.md).

## Use the CLI

Client configuration contains `endpoint`, `workspace`, and `token_file`, and is written with owner-only permissions. Override these with `--endpoint`, `--workspace`, `--token-file`, or `AQ_GRAPH_ENDPOINT`, `AQ_GRAPH_WORKSPACE`, `AQ_GRAPH_TOKEN_FILE`. `AQ_GRAPH_TOKEN` supplies a token through the environment when no file is configured. Tokens never belong in command arguments, graph content, or logs. A token file must have no group/other permissions (`chmod 600`).

For a hosted workspace, set the endpoint and workspace and initialize a separate configuration. `init` writes only client configuration; it never creates a second authoritative database or changes a hosted workspace:

```sh
aq-graph --config "$HOME/.config/aq-graph/hosted.json" \
  --endpoint https://graph.example.com --workspace WORKSPACE_UUID \
  --token-file "$HOME/.config/aq-graph/hosted-token" init
```

Use IDs and current `version` values from responses in subsequent commands:

```sh
aq-graph task show TASK_UUID --context --depth 2 --limit 50
aq-graph task tree TASK_UUID --depth 2 --limit 50
aq-graph task set-parent CHILD_UUID --parent PARENT_UUID --expected-version 1
aq-graph task set-parent CHILD_UUID --root --expected-version 2
aq-graph task update TASK_UUID --expected-version 1 --title "Implement login and logout"
aq-graph task activate TASK_UUID --expected-version 2
aq-graph task complete TASK_UUID --expected-version 3
aq-graph task reopen TASK_UUID --expected-version 4
aq-graph task cancel TASK_UUID --expected-version 5
aq-graph task list --limit 20
aq-graph task ready --limit 20

aq-graph knowledge create --title "Authentication decision" --type decision --file decision.md
aq-graph task update TASK_UUID --expected-version 1 --provenance-file sources.json \
  --valid-from 2026-01-01T00:00:00Z --valid-until 2027-01-01T00:00:00Z \
  --review-at 2026-11-01T00:00:00Z
aq-graph knowledge update KNOWLEDGE_UUID --expected-version 1 --clear-valid-until --clear-review-at
aq-graph knowledge verify KNOWLEDGE_UUID --expected-version 1 \
  --evidence-file evidence.json --reason "Confirmed against source"
aq-graph knowledge retire KNOWLEDGE_UUID --expected-version 2 --reason "Replaced procedure"
aq-graph knowledge restore KNOWLEDGE_UUID --expected-version 3 --revision 1 --include-lifecycle
aq-graph link add TASK_A_UUID TASK_B_UUID --type blocks
aq-graph link add TASK_UUID KNOWLEDGE_UUID --type references --target-revision 1 \
  --description "Decision evidence"
aq-graph link remove EDGE_UUID --expected-version 1
aq-graph search "authentication"
aq-graph search "authentication" --kind task --valid-at 2026-10-30T00:00:00Z
aq-graph task list --review-due-at 2026-11-01T00:00:00Z --include-retired
aq-graph search "authentication" --include-retired --include-disputed
aq-graph history RECORD_UUID --limit 20
aq-graph revision RECORD_UUID --revision 2
aq-graph diff RECORD_UUID --from 1 --to 2
aq-graph task restore TASK_UUID --expected-version 6 --revision 1
```

Tasks and knowledge use the same revision engine. Every meaningful content, lifecycle, relationship-membership, parent or deletion change advances the affected record’s logical revision and CAS version; no-ops create no duplicate revision. The wire field `content_revision` is retained for compatibility and now identifies this complete logical record revision. `revision ID --revision N` returns the exact node snapshot plus its recorded relationship set; `diff` compares both node fields and relationships.

Both `task restore` and `knowledge restore` restore content and recorded relationships as a new revision. Their default preserves current task state, retirement state/reason and deletion tombstone. Administrators can add `--include-lifecycle` to deliberately restore historical lifecycle/tombstone state for either kind. Verification remains subject to current policy on both kinds: a meaningful restore clears verification and never revives a historical badge or old administrative authority; a true no-op preserves current verification. History is never rewritten. Delete tombstones preserve identity and history; attached active edges must be explicitly removed first. Both kinds start with `content_type:note` and `verification:unverified`. Verification/dispute and retirement require admin authority and a nonempty reason for either kind. A meaningful protected correction clears verification; a no-op preserves it. Default restore of retired knowledge keeps it retired; use `--include-lifecycle` with an active historical revision when intentionally unretiring it. Ordinary task edits remain collaborative for workspace writers; unverified knowledge edits require its author or an admin. Protected or retired records require admin authority for either kind. Lists and search exclude retired records by default; search also excludes disputed records unless requested. Plain lists retain disputed records; direct authorized reads retain them.

Both kinds support typed provenance arrays through `--provenance-file`, and verification takes the same descriptor shape through `--evidence-file`. For example:

```json
[{"source_id":"decision-review","kind":"url","url":"https://example.com/review","observed_at":"2026-10-30T00:00:00Z","retained":false}]
```

Revision relationships include all outgoing edges and incoming structural edges (`blocks` and `parent-child`), so task prerequisites and parent membership are versioned with their record. Structural edge changes advance both endpoints; informational changes advance the source only. Incoming informational links remain derived from other records. Floating targets resolve current content, and pinned targets retain exact revision semantics; a record snapshot is not whole-graph time travel.

Pre-schema-6 snapshots remain readable without fabricated relationships: `relationships_available:false` and `links:null` mean the old relationship set is unknown. Restore rejects such an incomplete historical revision. Migration 0006 appends an attributed baseline for current state and current relationship membership, preserving every old snapshot and event. Use the new baseline or a later complete revision for restoration. A restore can change at most 100 relationships and rechecks current authority, endpoint access and graph invariants; an old valid snapshot may now require reconciliation.

The same `--type`, `--provenance-file` (`--sources-file` alias), `--valid-from`, `--valid-until` and `--review-at` options apply to task and knowledge create/update. Dates must be RFC3339 with a timezone; update `--clear-valid-from`, `--clear-valid-until` and `--clear-review-at` explicitly send JSON null. Classification uses canonical `content_type`; `knowledge_type` remains an accepted legacy API input alias. Task execution state and acceptance criteria remain task-only.

Validity is descriptive: `valid_at` uses a half-open interval (from inclusive, until exclusive), and absent bounds are unbounded. `review_due_at` selects nonempty review markers at or before the cutoff. Both lists and search accept these filters; dates do not schedule work, create deadlines or change readiness. Search covers both kinds by default.

Shared verification, summaries and retirement use the same operations:

```sh
aq-graph task verify TASK_UUID --expected-version 2 --evidence-file evidence.json --reason "Reviewed assertion"
aq-graph summary RECORD_UUID --expected-version 3 --source-revision 3 --file summary.txt
aq-graph summary RECORD_UUID --expected-version 4 --source-revision 4 --clear
# An existing successor→old supersedes edge is required:
aq-graph task retire OLD_UUID --expected-version 5 --reason "Replaced" --successor SUCCESSOR_UUID
# Or add that new edge and retire atomically:
aq-graph knowledge retire OLD_UUID --expected-version 5 --reason "Replaced" \
  --successor SUCCESSOR_UUID --link-successor
```

A summary must name the current logical revision before it is saved, so summarizing revision 3 creates revision 4 with `summary_of_revision:3`. It is nonblank UTF-8 text of at most 8192 bytes; no model runs in the service. Other meaningful source, content, verification, lifecycle, date or relationship changes clear the current summary and its pin. Historical snapshots retain it. An unchanged summary/pin or clearing an absent summary is a no-op after CAS/authority checks.

Retirement preserves task execution state. A successor must be a distinct nondeleted record in the same workspace, with a `supersedes` edge directed successor→old; either endpoint may be task or knowledge. Existing retired successors permit forwarding chains, but cycles and removing a required edge or successor are refused unless pointers are corrected in the same batch. `--successor` uses an existing edge; `--link-successor` adds a new edge atomically and fails if that informational edge already exists. Retirement never completes a prerequisite or bypasses parent guards.

Task parent changes are atomic and version checked. Each child has one live parent; the hierarchy has at most three levels and cannot jointly cycle with blocking edges. Completed parents refuse new children, and normal completion refuses open direct children. `task tree` returns structural children with truncation metadata. Informational edges do not affect readiness. Structural links cannot pin content revisions; see the API for the endpoint-kind matrix.

Every command accepts global `--json`; success goes to stdout and errors to stderr. Human mode prints formatted JSON with all version and pagination metadata. List responses contain `next_cursor`; pass it using `--cursor`. Context discloses `truncated`. Limits and schemas are documented in the [API contract](docs/api.md) and [OpenAPI document](docs/openapi.json). The client sends API version 1 and rejects incompatible successful responses.

Exit categories are stable: `0` success, `1` local/internal failure, `2` invalid input, `3` missing record, `4` conflict, `5` unauthenticated, `6` forbidden, `7` unavailable/timeout/protocol failure. HTTP details remain in the structured error code. Retry an uncertain mutation with the **identical payload and idempotency key**. Friendly mutations accept `--idempotency-key`; a generated key appears in errors if a request fails, including timeout. Rerunning without that key creates a new operation. Clients must re-read and deliberately revise stale writes rather than overwrite them.

## Atomic batches and drafts

`mutate --file request.json` takes a `MutationRequest`; `--file -` reads stdin. For example:

```json
{
  "idempotency_key": "plan-authentication-001",
  "reason": "Approved implementation plan",
  "operations": [
    {"op":"create_node","kind":"task","title":"Implement authentication","task_state":"draft"}
  ]
}
```

A draft file has the same fields plus a `name`. Expected versions are part of update/delete operations in the file. Drafts stay outside live state until atomic application:

```sh
aq-graph changeset create --file proposal.json
aq-graph changeset inspect DRAFT_UUID
aq-graph changeset validate DRAFT_UUID
aq-graph changeset apply DRAFT_UUID --idempotency-key apply-approved-plan-001
```

Validation rolls proposed writes back. Application rechecks permissions, versions, references and graph invariants against current state. The `draft` alias is also available. No silent semantic merging occurs.

## Build, verify, operate

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
# Use a dedicated test database; integration tests never use AQ data.
AQ_GRAPH_TEST_DATABASE_URL=postgres://USER@127.0.0.1/aq_graph_test cargo test --locked --all-targets
cargo build --locked --release
```

CI runs Rust checks, PostgreSQL-backed tests, an image build, a Compose smoke test, and isolated backup restoration. Tag releases build Linux x86_64 and macOS arm64 CLI archives with SHA-256 checksums and a versioned GHCR image. The Linux release artifact targets glibc on Ubuntu 22.04 or newer. Windows and other targets are not yet release-tested. API v1 and schema migration `0007` are the initial compatibility contract. Schema-breaking upgrades need an explicit migration and recovery plan; never assume an older image can read a newer schema.

The [shared node contract](docs/specs/0004-shared-node-model.md) defines the common field and validation rules.

See [operations](docs/operations.md) for credentials, backup, restore, upgrades and failure handling, and [benchmarking](docs/benchmark.md) for reproducible fixtures, mixed load and limitations. The [specification](docs/specs/0001-aq-graph.md) describes intended scope. Semantic search, AQ execution, whole-graph time travel, offline merging and permanent erasure are outside this baseline.

The [AQ integration contract](docs/aq-integration.md) and [example adapter](examples/agent_q_adapter.py) show how AQ can consume the public API without direct database access.
