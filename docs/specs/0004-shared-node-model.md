# Shared node model

Status: implemented by schema 0007. AQ v2 is a fresh start; this specification does not add a v1 importer or promise complete v1 compatibility. Optional transfer from a private v1 installation belongs in an external one-off script.

## One record, common revision rules

Tasks and knowledge use the same `Node`, current-state table, mutation engine, revision snapshots, evidence descriptors, and CLI operations. `kind` selects behavioral validation. Shared fields are title, body, labels, namespaced metadata, `content_type`, provenance, verification attribution, summary and its source revision, validity/review dates, retirement reason, and optional successor. New nodes default to classification `note` and verification `unverified`.

`content_type` accepts fact, decision, policy, procedure, incident, reference, and note for either kind. Current and historical API node responses use this name. The previous `knowledge_type` input spelling remains accepted; mutation normalization retains that internal spelling to preserve existing idempotency hashes. This is internal service evolution, not a v1 import facility.

Tasks additionally have execution state and acceptance criteria. Knowledge rejects those task-only fields. Completing a task does not verify its assertions; verification does not complete it. Creation cannot assert verified status. Both kinds support explicit `verify_node` with administrator authority, a reason, and named evidence. Evidence references are validated descriptors, not claims that remote material was fetched or retained.

Every meaningful durable mutation advances both `version` and `content_revision`, including summaries, verification, dates, retirement, and owned relationships. Ordinary content, lifecycle, and relationship changes reset verification for both kinds. Idempotent replay and true no-op updates preserve versions. Current reads use indexed current-state tables; historical snapshots are not read to answer ordinary graph queries.

## Derived summaries

`set_summary` requires node ID, expected version, source revision, and text or null to clear. Text must be nonblank and at most 8192 bytes. A new summary must refer to the current content revision. Saving it creates the next revision, retaining an explicit foreign key to the same node's source revision. It also resets verification, because the new summary is a new assertion.

For example, summarizing revision 12 creates revision 13 with `summary_of_revision: 12`. Any other meaningful mutation clears the current summary and its pin, including verification, lifecycle changes, owned link changes, and restoration. Historical summaries remain in historical snapshots. An identical summary with the same pin, or clearing an already absent summary, is a no-op after the expected-version and permission checks. Concurrent agents cannot attach an old generated summary to newer material.

There is no background summarizer or model call in aq-graph. Callers generate summaries and submit them with the revision they read.

## Validity and review metadata

`valid_from`, `valid_until`, and `review_at` are optional RFC3339 timestamps normalized to UTC microsecond precision. The validity interval is half-open: from inclusive, until exclusive. Missing bounds are unbounded; when both exist, from must precede until. Review time is an independent reminder, not a validity bound.

PATCH omission preserves these values; JSON null clears them. List and search support `valid_at` and `review_due_at`; the latter includes only nodes with a nonnull review time at or before the supplied cutoff. These filters apply equally to both kinds. Dates are descriptive metadata and do not change execution state or readiness automatically. Task deadlines, scheduling, review queues, and reminders belong in the caller.

## Retirement and successors

Retirement is a shared administrator operation requiring a nonblank reason. It preserves content and task execution state. Lists and search exclude retired records by default, with explicit opt-in. Search includes both kinds unless `kind` is supplied and excludes disputed records unless requested; ordinary lists retain disputed records. Direct reads and history still expose retired records.

Ready tasks exclude retired nodes and descendants of retired parents. Retirement does not complete a prerequisite or waive a parent completion guard. An unfinished retired prerequisite continues to block dependent tasks.

A retired node may name a `successor_id`. It must be a distinct, nondeleted node in the same workspace and have a live `supersedes` relationship from successor to retiring node. Successors may themselves be retired, allowing forwarding chains, but pointer cycles are refused. `supersedes` accepts either node kind at each endpoint. Other relationship endpoint rules remain unchanged.

Pointer and edge consistency is validated against the final atomic batch. Removing a required supersedes edge or deleting a named successor is refused unless the affected pointer is also corrected in that batch. Scoped foreign keys and indexes support the checks. Only affected nodes, their incoming pointers, and reachable successor chains are inspected; unrelated content writes do not scan the graph. Concurrent structural and successor writes use serializable transactions with bounded retries.

Retired content cannot be edited in place. Administrators may correct retirement reasons or successor pointers; unretirement uses explicit lifecycle restoration. The CLI's `retire --successor` uses an existing supersedes edge; `--link-successor` creates the edge and retires atomically.

## Authority and restoration

Ordinary task edits remain available to workspace writers for multi-agent collaboration. Unverified knowledge retains creator-or-admin editing. Verified, disputed, and retired nodes require an administrator for either kind. Structural edits check the authority of the affected revision owners, including both endpoints and old/new parents.

Default restore preserves task state, retirement reason/state, successor pointer, and deletion state. Restoring historical lifecycle explicitly with `include_lifecycle: true` requires an administrator for either kind. Knowledge restoration remains administrator-only. Restore validates current graph constraints and successor integrity, resets verification on meaningful change, and clears the current summary instead of reinstating a stale derived assertion. A true no-op preserves both verification and summary.

## Existing aq-graph installations

Migration 0007 renames the classification column, adds typed nullable fields and integrity constraints, and supplies explicit common classification/verification defaults to current task records. Each changed task receives an attributed baseline revision reusing its exact current relationship set. Existing snapshot payloads and history remain immutable; older snapshots decode through the prior field alias and nullable defaults. Old task classifications are normalized to `note` when restored into the new current-state contract.

Normal numbered database migrations remain part of operating aq-graph. They are unrelated to importing v1 application data.

## Validation

Exercise the shared contract on tasks and knowledge: evidence and verification independent of completion, summary CAS/source pins/no-op/invalidation, validity boundaries and null clearing, list/search filters, retirement and readiness, replacement chains and atomic rollback, preview/restoration, and workspace authority. Migration tests must preserve historical payloads and exact relationship sets while rejecting invalid current data. Keep concurrency checks and current-query measurements separate from claims about production capacity.
