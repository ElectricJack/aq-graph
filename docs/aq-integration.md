# Agent Q integration boundary

The standalone service can be integrated through the [v1 API](api.md). The [Python adapter example](../examples/agent_q_adapter.py) demonstrates persistent HTTP connections, graph-ready candidates, bounded task context, and version-checked completion. It has no dependency on AQ internals and does not implement scheduling or execution claims.

Configure an AQ service identity scoped to its workspace. AQ retains the authenticated identity for the service operation; a task/session identifier supplied as provenance is an attribution, not a different authorized actor. Do not reuse a human administrator token for normal worker access.

## Execution and retry behavior

AQ fetches graph-ready candidates, applies its own policy and resource constraints, and claims execution in its own authority. A ready response alone never authorizes two workers to execute the same task. Fetch current context before beginning work and reconcile graph changes according to AQ's execution policy.

For completion, AQ persists the intended operation, expected version, and idempotency key in its own durable delivery record before sending the request. It marks delivery complete after a successful response. On a timeout or disconnected response, replay the exact request with the same key. A version conflict requires reconciliation; do not fetch a new version and blindly repeat a completion that may no longer be valid.

AQ's execution database and aq-graph do not share a transaction. Use this durable delivery pattern to make completion retryable. It does not turn external Git, shell, or provider actions into exactly-once operations. Record evidence references separately from deciding whether an execution succeeded.

## Fresh-start integration

Agent Q v2 starts with a new workspace and new records. There is no v1 cutover or historical-data import prerequisite. The example adapter and published API establish the boundary; production AQ wiring and validation of v2 workflows remain integration work.

aq-graph does not ship or support a v1 importer, legacy-history ingestion API, v1 database adapter, reconciliation/cutover tool, or migration service for other installations. V1 tests remain useful behavioral evidence, but full v1 parity is not a release criterion.

If the owner later chooses to transfer selected v1 data, use a separately maintained external one-off script outside this repository. Ordinary API writes create new history under the authenticated actor; they must not be presented as preserving original historical actors or timestamps. The optional script may retain legacy references as attributed data, without adding v1-specific semantics to the service.

Normal aq-graph schema upgrades, backup/restore, and node revision restoration remain supported. They manage data already owned by aq-graph and are distinct from importing a previous product. The existing v1 installation and database remain unchanged.
