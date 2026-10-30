# Agent Q integration boundary

The standalone service can be integrated through the [v1 API](api.md). The [Python adapter example](../examples/agent_q_adapter.py) demonstrates persistent HTTP connections, graph-ready candidates, bounded task context, and version-checked completion. It has no dependency on AQ internals and does not implement scheduling or execution claims.

Configure an AQ service identity scoped to its workspace. AQ retains the authenticated identity for the service operation; a task/session identifier supplied as provenance is an attribution, not a different authorized actor. Do not reuse a human administrator token for normal worker access.

## Execution and retry behavior

AQ fetches graph-ready candidates, applies its own policy and resource constraints, and claims execution in its own authority. A ready response alone never authorizes two workers to execute the same task. Fetch current context before beginning work and reconcile graph changes according to AQ's execution policy.

For completion, AQ persists the intended operation, expected version, and idempotency key in its own durable delivery record before sending the request. It marks delivery complete after a successful response. On a timeout or disconnected response, replay the exact request with the same key. A version conflict requires reconciliation; do not fetch a new version and blindly repeat a completion that may no longer be valid.

AQ's execution database and aq-graph do not share a transaction. Use this durable delivery pattern to make completion retryable. It does not turn external Git, shell, or provider actions into exactly-once operations. Record evidence references separately from deciding whether an execution succeeded.

## Migration preparation

No existing AQ project is cut over by this repository. The pilot needs a selected project, its exported schemas and history, a mapping report, and a maintenance window. The current generic mutation endpoint creates new history under the importing principal. It is not a legacy-history importer and must not be described as preserving original audit identities or timestamps.

Before a pilot, implement and review a dedicated migration format that preserves available record IDs or durable mappings, revision IDs, timestamps, provenance, edge directions, and tombstones. Reject unsupported legacy dependency types. Do not synthesize historical facts from current snapshots. Validate the import in an isolated database and compare record counts, relationships, revision references, readiness, and representative context queries against the source.

Freeze writes in the selected source workspace before final reconciliation. After cutover, route authoritative graph writes to aq-graph only. If new writes occur after cutover, rollback requires reverse export and reconciliation rather than simply switching to an older source database.

The example adapter and API establish the integration boundary. Production AQ adapter wiring, legacy import/export, and the actual pilot remain a separate deployment milestone; the existing AQ code and database are unchanged.
