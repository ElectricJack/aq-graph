# aq-graph API v1

The [OpenAPI 3.1 contract](openapi.json) publishes every route, request, tagged mutation variant, and response shape. The service and CLI operate independently of Agent Q. All writes and authoritative reads use this API.

Send `Authorization: Bearer TOKEN` and `Content-Type: application/json` for JSON bodies. Credentials belong to one workspace and authenticate an actor with `read`, `write`, or `admin` capability. Read permits queries; write adds graph mutations and draft operations; admin adds credential management. A claimed actor field in mutation input is rejected. Tokens must contain 32–4096 bytes, are stored only as SHA-256 hashes, and must be supplied through the configured environment or protected file. The initial workspace/admin is provisioned by the local `init` command against configured PostgreSQL; bootstrap never replaces an existing workspace's credentials.

Every response carries `X-AQ-Graph-API-Version: 1`. Clients may send the same header; an unsupported value returns HTTP 400 with `unsupported_api_version`. The `/v1` path is the API major version. Unknown JSON fields are rejected in mutation, patch, draft, credential, and apply bodies. UUIDs are opaque identities, and timestamps are server-generated RFC 3339 UTC values. Versions and content revision IDs are positive 64-bit integers; treat them as opaque tokens, not timestamps or global commit order.

All workspace routes start with `/v1/workspaces/{workspace}`. The table below omits that prefix.

| Method | Path | Request / result |
| --- | --- | --- |
| GET | `/nodes` | Optional `kind=task\|knowledge`; paginated current records |
| GET | `/nodes/{id}` | Current node, including a tombstone when deleted |
| GET | `/nodes/{id}/context` | Root node, bounded neighbors, edges, pinned snapshots, truncation flag |
| GET | `/nodes/{id}/history` | Paginated immutable content revisions, lifecycle events, and related edge changes |
| GET | `/nodes/{id}/diff?from=1&to=2` | Full before/after snapshots and changed durable fields |
| GET | `/ready` | Paginated intrinsically ready active tasks |
| GET | `/search?q=authentication` | Paginated PostgreSQL full-text knowledge search |
| POST | `/mutations` | Atomic ordered batch; `MutationRequest` |
| POST | `/changesets` | Persist a named proposal; `DraftRequest` |
| GET | `/changesets/{id}` | Proposal, author, operations, status, and applied result |
| POST | `/changesets/{id}/validate` | Execute and roll back a proposal; return validated before/after preview |
| POST | `/changesets/{id}/apply` | `{"idempotency_key":"..."}`; revalidate and commit complete proposal |
| POST | `/credentials` | Admin: `{"actor_id":"agent","token":"32-or-more-random-bytes...","capability":"write"}` |
| DELETE | `/credentials/{actor}` | Admin: revoke actor; last administrator must remain |

`GET /health/live` is unauthenticated and reports process liveness. `GET /health/ready` is unauthenticated and returns 200 only when PostgreSQL is reachable and applied migrations exactly match the binary's supported versions and checksums; otherwise it returns 503 `not_ready`. All success responses use HTTP 200 and JSON. Credential success is `{"ok":true}`.

## Mutations and optimistic concurrency

A mutation envelope contains `idempotency_key`, optional `reason`, and 1–100 `operations`. The batch is all-or-nothing, executes operations in order, and can reference records created earlier in the same batch. The result is `{"change_set_id":"UUID","results":[...]}` with one resulting node or edge per operation. Required history and the idempotency receipt commit with current state.

```json
{
  "idempotency_key": "implement-login-1",
  "reason": "Approved implementation plan",
  "operations": [
    {
      "op": "create_node",
      "kind": "task",
      "title": "Implement login",
      "body": "Use the authentication decision linked to this task.",
      "task_state": "active",
      "metadata": {"aq:priority": 1}
    }
  ]
}
```

| `op` | Required fields | Optional fields |
| --- | --- | --- |
| `create_node` | `kind`, `title` | `id`, `body`, `acceptance_criteria`, `labels`, `metadata`, `task_state`, `knowledge_type`, `verification`, `provenance` |
| `update_node` | `id`, `expected_version`, `patch` | Patch can set content fields, lifecycle, knowledge fields, or `retired` |
| `delete_node` | `id`, `expected_version` | None |
| `restore_node` | `id`, `expected_version`, `revision` | None |
| `add_edge` | `source`, `target`, `edge_type` | `id`, `source_revision`, `target_revision` |
| `remove_edge` | `id`, `expected_version` | None |

Task states are `draft`, `active`, `completed`, and `canceled`. Knowledge types are `fact`, `decision`, `policy`, `procedure`, `incident`, `reference`, and `note`; verification is `unverified`, `verified`, or `disputed`. A task defaults to draft. Knowledge defaults to note/unverified. Task and knowledge fields cannot be mixed.

A node's `version` changes for each authoritative mutation to that node. A lifecycle-only mutation appends an event without creating a full content revision. Content edits create full immutable snapshots and advance `content_revision`. A combined edit records both. Omitted or null patch fields leave their values unchanged. A no-op patch creates neither a new version nor duplicate history. `restore_node` creates a new content revision from an old snapshot and preserves current task lifecycle; it never erases intervening history. Deletion retains identity, tombstone, and history. Active incident edges must be removed explicitly before deleting a node, which can occur in the same atomic batch.

`A blocks B` means B depends on A. Both endpoints of `blocks` and `parent-child` must be tasks. Each structural family is acyclic independently. Concurrent competing insertions cannot both commit a cycle. Parenthood does not block readiness. An active task is ready only when all blocking prerequisites are completed; a canceled prerequisite remains unresolved. Readiness conveys graph state and is not a worker claim.

Informational edge types are `references`, `motivated_by`, `produces`, `supports`, `contradicts`, and `supersedes`. These can pin endpoint content revisions. Structural edges use current task state and reject revision pins. A missing pinned revision returns an explicit error; retrieval never substitutes current content. Edge addition and removal produce immutable history and do not change endpoint node versions.

Idempotency is scoped by authenticated actor, workspace, and operation (`mutate`, draft creation, or draft application). Repeating a key with the same parsed payload returns the committed outcome, including IDs; reusing it with another payload returns 409. JSON object key order is immaterial. Draft application includes the draft identity in the payload, so reusing the key for another draft conflicts. Keys contain 1–128 bytes. Pilot receipts are retained indefinitely; no automated receipt or history pruning runs. Clients can retry after a lost connection or restart with the same key. A timeout can occur after a commit, so always reuse the original key rather than inventing another one. Different keys remain distinct operations. Expected-version conflicts never overwrite newer content.

Draft creation takes the same batch fields plus a `name` of 1–200 bytes. Draft-generated IDs remain stable across validation and application. Drafts cannot affect live readiness. Validation executes the actual mutation logic in a transaction and rolls back all writes, including history; successful validation is a preview, not authorization to bypass checks during application. Application rechecks credentials, expected versions, endpoints, and invariants. A stale draft returns a conflict and remains unapplied. Large imports and graph branches are outside this contract.

## Queries, history, and limits

List, ready, search, and history queries take `limit` (default 50, range 1–200) and an opaque `cursor`. Pages contain `items` and `next_cursor` (null when exhausted). Pass the returned cursor unchanged with the same filters. Current-record lists use stable UUID order; history uses an internal sequence cursor. Pagination is not a whole-graph snapshot or change subscription. Concurrent commits may change later pages, and sequence allocation is not commit order.

History entries contain `sequence`, `entry_id`, `category` (`revision`, `lifecycle`, or `edge`), authenticated `actor_id`, `operation`, optional `reason`, `change_set_id`, `data`, and `created_at`. Change-set IDs group atomically committed edits. A history query for a node includes incident relationship changes. Diff takes node-specific content revision identifiers and reports durable field changes. It does not undo lifecycle transitions or reconstruct a graph at a historical time.

Context query parameters are `depth` (default 1, range 0–5), `limit` (default 50, range 1–200, including the root), `max_bytes` (default 262144, range 1024–1048576), and `direction` (`both`, `incoming`, or `outgoing`; default both). Its shape is:

```json
{"node":{},"nodes":[],"edges":[],"pinned_revisions":[],"truncated":false}
```

`nodes` excludes the root. Context uses one consistent committed database snapshot and batched database-side adjacency reads. Edge count is capped at four times the requested record limit. Record, edge, and byte caps disclose omissions with `truncated:true`. The byte budget covers the serialized JSON envelope and pinned snapshots. A root larger than the requested budget returns 400 with guidance to increase it. A depth boundary defines the requested neighborhood; it does not itself imply truncation.

Search uses PostgreSQL English full-text matching, restricted to current nondeleted knowledge in the authenticated workspace. Search text contains 1–1024 bytes. Retired knowledge remains searchable; deleted knowledge does not. Semantic search and synthesis are not part of v1.

Server caps are 256 admitted requests, a 1 MiB request body, a 2 MiB serialized response, and a ten-second deadline covering body collection, authentication, pool wait, database work, and response serialization. Excess admission is rejected immediately without an unbounded queue. Database pools have 1–128 connections (default selected by deployment) and a two-second acquisition deadline. Mutation transactions use four-second statement, two-second lock, and five-second idle-transaction deadlines. Each mutation transaction attempt has a six-second overall deadline; serializable/deadlock retries are bounded to four attempts within the service request deadline. A large page may hit the response cap; reduce its limit. A mutation whose result would exceed 2 MiB is rejected with 400 `validation` before commit. JSON body errors are structured, including invalid content types and malformed fields.

Content caps are title 512 bytes, body 65536 bytes, acceptance criteria 16384 bytes, 64 labels of up to 128 bytes, and namespaced metadata up to 16384 bytes. Provenance is up to 64 objects and 16384 bytes. Mutation reasons are up to 4096 bytes. Store large artifacts externally and place stable references in content. Never put credentials or secrets in versioned records.

## Errors, overload, and shutdown

Errors always have this shape:

```json
{"error":{"code":"conflict","message":"expected version does not match current state"}}
```

| HTTP | Stable code | Meaning |
| --- | --- | --- |
| 400 | `validation` | Malformed JSON, invalid UUID/query/fields, graph cycles, unavailable requested revision, or invalid domain input |
| 400 | `unsupported_api_version` | Requested header version unsupported |
| 401 | `unauthorized` | Missing, revoked, or wrong-workspace bearer credential |
| 403 | `forbidden` | Authenticated actor lacks capability |
| 404 | `not_found` | Record or route unavailable in this workspace |
| 405 | `method_not_allowed` | Unsupported route method |
| 409 | `conflict` | Stale version, unresolved references, duplicate identity, applied draft, or key payload mismatch |
| 413 | `body_too_large` | Request exceeds server byte cap |
| 413 | `response_too_large` | Response exceeds server byte cap |
| 500 | `internal` | Internal failure; SQL, connection strings, and driver details withheld |
| 503 | `overloaded` | Admission/pool/transaction capacity exhausted |
| 503 | `timeout` | End-to-end request deadline exceeded |
| 503 | `not_ready` | Health readiness schema/database check failed |

Every 401 includes a bearer `WWW-Authenticate` challenge. Every 503 includes `Retry-After: 1`. Apply jittered backoff and keep mutation keys and payloads unchanged. Error messages are explanatory; automate on status and code. Logs contain method, matched route template, status, and elapsed service time; they omit credentials, request payloads, concrete paths, and search query values. End-to-end client latency additionally includes network and response transfer. SIGINT/SIGTERM stop acceptance and drain requests; committed outcomes remain retryable after restart. Operational log retention is managed by deployment. Admission covers request processing through response buffering, then releases before socket transmission completes. The baseline server does not cap accepted TCP connections or total memory retained by slow socket consumers. Production ingress must set connection caps and header, read, write, and idle deadlines; those network protections are deployment responsibilities.

## Running the contract tests

Run a dedicated PostgreSQL instance or database, then:

```sh
AQ_GRAPH_TEST_DATABASE_URL=postgres://aqgraph@127.0.0.1:5432/aq_graph_test cargo test --all-targets
```

The suite applies real migrations and creates a fresh workspace and credentials per test. It fails clearly if the environment variable is missing or PostgreSQL is unavailable; it never silently skips database assertions. Tests cover workspace/capability isolation, revocation, JSON/body errors, selective history and pagination, restore, competing updates, concurrent same-key retries, both structural cycle races, readiness semantics, draft rollback/application/staleness, atomic batch rollback, restart persistence, pinned content, deletion/search exclusion, and context limits. Transport unit tests cover overload, deadline cancellation/admission release, and serialized response caps.
