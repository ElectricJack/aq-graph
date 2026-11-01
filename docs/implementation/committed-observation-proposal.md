# Committed Graph observations for AQ waits and policy events

Status: source-audited proposal, not an implemented or published capability.
Base: aq-graph `225329cd6acf5a775fd2a0b016dd1c45fb16c79b`. No code, dependency,
DDL, canonical service, schema or graph data change is included. The repository
has no root AGENTS.md. Changes here are isolated from the existing uncommitted
integration documentation in the main checkout.

## Problem and actual source boundary

AQ needs to register a wait, sleep, and recover a task/dependency transition even
when that task subsequently reopens. Current-state polling can miss the terminal
interval. AQ's own mutation outbox sees only AQ-originated requests, while other
clients can write through Graph's CLI/API. Neither supplies a Graph commit log.

Source findings:

- `migrations/0001_initial.sql` gives history_entries a global PostgreSQL IDENTITY
  sequence. `Store::history` (`src/store.rs:536`) orders/filter-pages by that
  allocation. Two independent transactions can allocate10/11, commit11 first,
  then commit10 after a reader has advanced past11. A timestamp, MAX(sequence),
  successful request, SERIALIZABLE isolation or immutable node revision is not
  a workspace-wide committed watermark.
- `mutate_once` and `apply_draft` atomically persist domain changes, history and
  idempotency receipts. Their receipt lock orders only one actor/operation/key,
  not all workspace writers. Content-only writes can use READ COMMITTED; there
  is no existing workspace commit head to reuse.
- `execute` covers the ordinary node/edge mutation variants, including restores,
  structural endpoint checkpoints and parent moves. CLI commands use the HTTP
  service. Public Store.pool and offline SQL fixtures can bypass application
  hooks; an AQ-only hook or hook in one HTTP handler is insufficient.
- Migration0008 already uses transactional node/edge triggers for the internal
  ready projection, including guard writes against stale snapshots. Its per-node
  guard is derived readiness data, not an observation cursor. It covers neither
  every field nor commit ordering and must retain its existing meaning.
- Since migration0006, content_revision is a complete logical-record revision:
  DURABLE_FIELDS includes task_state/deleted, and structural link changes also
  advance endpoint revisions. Completion normally advances both version and
  content_revision. Existing fields must not be relabelled content-only.
- One atomic batch can complete and then reopen a node before commit. Existing
  history/revision rows can retain these intermediate operations. They are not
  proof that a completed graph state was externally committed. New replay must
  coalesce first-before and final-after per object within a commit group.

The process currently listening on4195 is the PostgreSQL Rust service binary
`eb9c8f41002cb5d9675f87de0cadbe34e7d74cfc60d2449cd23ac4df76921fea`, not a new
observation owner. Its existing routes expose no committed journal or registration
boundary. Treat it, old binaries, custom peers and fixtures as **unsupported**
until their exact new protocol/backend semantics pass qualification. No timestamp
or old history sequence is upgraded to a receipt by the AQ adapter.

## Ownership and smallest correct storage mechanism

Graph owns ordering and historical transition facts. AQ owns waits, condition
policy, scheduling, wake admission and execution authority. Workers receive AQ's
normal continuation/fresh-grant protocol; they never copy the Graph database or
use a Graph event to resume themselves. Events are hints until AQ obtains and
validates the owning source evidence. Playbooks consume a registered source event;
this adds no ordinary playbook commands or worker-to-Graph tunnel.

Recommended PostgreSQL owner: add a workspace-scoped transactional head and a
coalesced journal. Use database node/edge triggers so API/CLI/Store writers and
ordinary DML use the same append boundary. No WAL decoder, broker, replication
service or worker database is required.

Proposed internal tables, in the next separately allocated **aq-graph** migration:

| Table | Essential key / state |
| --- | --- |
| graph_observation_heads | workspace_id PK/FK; durable stream_id UUID; generation positive bigint; last committed-group sequence; last group UUID; retained floor; coverage status |
| graph_observation_batches | workspace/stream/generation/sequence PK; unique group UUID; transaction key; retained/gapped kind; fact count; optional original Graph change-set attribution |
| graph_observation_facts | group/object-kind/object-id PK; first before projection and final after projection; indexes for workspace/object/sequence pull |
| graph_observation_subscriptions | workspace/actor/subscriber UUID PK; immutable request digest, selector/pin/condition digest, initial snapshot+cursor/receipt, original absolute expiry; current lease/control disposition |

Keep all old tables, wire fields, revisions, history cursors and quotas intact.
Do not allocate AQ SQL38; Graph has its own migrations, currently0001–0008. The
next number and migration's deployment/rollback requirements require Graph owner
coordination before implementation.

For the first changed node/edge in a workspace/SQL transaction, a trigger obtains
a real write lock on that workspace's head and allocates sequence+1 transactionally.
It inserts one batch keyed to that transaction, retaining the head lock to commit
or rollback. Later changes in the same transaction reuse the batch. No IDENTITY,
nextval, txid value or wall-clock timestamp is used as a commit cursor. A database
transaction ID may identify the private in-flight batch, not determine ordering.

An object fact retains its first before state and overwrites only its final after
state in that same transaction. Each fact/group becomes visible only with the
domain commit. Transactions on another workspace do not contend on this head.
The head lock means a later same-workspace group cannot allocate/publish after a
still-uncommitted earlier group and then overtake it. Rollback/savepoint rollback
also rolls back the head, group and facts. PostgreSQL deadlock/serialization
failures remain retryable; no partial state is acknowledged.

Trigger coverage includes INSERT/UPDATE/DELETE of nodes and edges, every changed
field, both structural endpoints, restore/reparent side effects and COPY which
runs ordinary row triggers. Adding an edge fact does not manufacture endpoint
node revisions; capture actual resulting rows. Internal ready projection writes,
read snapshots, drafts, validation, receipts and credentials do not masquerade as
node/edge changes. A draft preview's transaction rollback leaves no journal entry.
Credential changes still fence reads using current authentication, independently
of domain change hints.

Workspace/node/edge identities are immutable in supported normal writes. A
new trigger must reject identity/workspace moves or explicitly fence both scopes;
it cannot attribute a cross-workspace SQL move to just one journal.

Unsupported bypasses are explicit: disabled triggers, replication-role bypass,
privileged DDL, arbitrary table replacement and uncoordinated restore. Reject or
fence TRUNCATE as a maintenance operation; do not silently skip it. Direct SQL
which violates normal Graph revision/domain rules must not produce a valid wait
transition proof merely because a trigger saw it. Such facts carry a typed domain
invariant failure and hold observation coverage until reconciled. Only qualified
writers/backend combinations may advertise the source profile.

Schema installation drains/excludes existing writers while seeding head rows and
installing triggers, as migration0008 does for derived state. Stale transactions
must retry rather than observe a missing/stale head and bypass capture. The exact
head upsert/guard and lock order need PostgreSQL interleaving tests before any
capability is published. No hand-waved claim about trigger/row-lock ordering is
sufficient. Observe existing4s statement/2s lock/6s transaction budgets and bounded
retry policies; benchmark the per-workspace serialization cost.

## Additive proposed HTTP protocol

Name: `graph-committed-observation-1`. Existing v1 API/version header and frozen
DTOs remain unchanged. All new documents use closed fields and strict UTF-8 JSON;
UUIDs and canonical positive decimal revision/sequence strings have no implicit
JavaScript integer conversion. Receipts are content-derived descriptors returned
by an authenticated owner, not signatures or standalone authority.

New routes under `/v1/workspaces/{workspace}`:

1. GET `/observations/capabilities`: explicit support/profile, storage scope,
   generation and finite limits. Absence/404/unknown profile means unsupported.
2. POST `/observations/subscriptions`: idempotent stable subscriber UUID plus
   immutable request/condition digest, exact node/edge selectors and starting
   revision pins, expected source scope and original absolute expiry. It records
   the source subscription and captures a current bounded snapshot and replay
   barrier **in one Graph transaction** under the same workspace head lock.
3. GET `/observations/subscriptions/{id}/changes?after=<cursor>&limit=16&max_bytes=262144`:
   bounded committed pull. Returns complete selected commit groups, a captured
   through boundary, next cursor, retention floor and explicit gap/expiry state.
4. DELETE `/observations/subscriptions/{id}`: idempotent release of control state,
   never removal of the immutable original receipt or domain history. No automatic
   extension of the original wait deadline. Renewal is deferred from slice1.

Read-authorized workspace service principals may observe only their workspace.
Subscription metadata allocation requires the same authenticated principal and
published finite per-actor/workspace control quotas; it grants no domain write
capability. Recheck exact current credential identity/capability in the owner
transaction, including rotation/revocation races, not just an old Principal's
actor label. A historical receipt cannot authorize a current read after access
loss. Scope/generation/address mismatch refuses before returning any object fact.

Proposed slice1 bounds:16 node/edge selectors per subscription,16 selected commit
groups/page,256KiB response,64KiB request and existing request/concurrency budgets.
Subscription and journal storage quotas must be finite and published; start with
configurable1024 active workspace subscriptions and256 per principal for tests,
then qualify the hundred-agent workload before setting production defaults.
Selectors sharing a workspace can be pulled centrally; do not poll once per
worker or duplicate the database. Current pilot idempotency receipts' existing
retention semantics are preserved; do not silently add cleanup to frozen APIs.

A scope contains workspace_id, durable stream_id and generation. It names storage
continuity, not a process boot or execution generation. A responder incarnation
may be diagnostic only; ordinary service restart must retain source continuity
and replay. Destructive restore/reseed/journal reset rotates generation before
publishing support; old cursors receive source_changed/gap. This is an operator
restore requirement, not something a database snapshot can prove by itself.

The registration receipt binds actor, subscriber, condition digest, exact
selectors/pins, source scope, original expiry, canonical snapshot bytes/digest and
committed cursor. Retrying the same subscriber/payload returns the original
receipt and expiry. A changed payload conflicts. An uncertain response is resolved
by the same stable subscriber or exact receipt lookup; never install a new watch
and silently start later. A snapshot can carry explicit absent/deleted/retired
facts; those must not be confused with access-denied or source failure.

Current facts and head are read under the held head lock, or a proven equivalent
consistent PostgreSQL snapshot. Pull reads head/floor/facts in one repeatable-read
snapshot, freezes through, and advances over nonmatching committed groups only
when their range is known complete. A cursor binds scope, sequence and anchor
UUID/digest; tampering, future positions and unknown anchors refuse. Commit groups
are never partially applied as authoritative predicates. Fixed bounded selectors
allow complete projected groups without transmitting full node bodies; stop
before a group exceeding a page budget and return an explicit response-limit
result, never advance over it. Exact limits require a worst-case restore/batch test.

### Exact proposed DTO joins

The new namespace uses the following closed records (all fields required unless
explicitly nullable); generated schema/examples are part of the code slice, not
published by this proposal:

- SourceScope = workspace_id UUID, stream_id UUID, generation decimal string.
- Cursor = scope, committed_sequence decimal string, group_id nullable UUID.
  Sequence0 requires null group; other positions require the owner's exact
  recorded anchor. Transport `after` is canonical compact sorted JSON encoded
  base64url without padding, bounded to1024 bytes. It is not a bearer token.
- Selector = member_id UUID, object_kind `task` or `dependency`, node_id plus
  starting_logical_revision for task; edge_id, edge_version, exact prerequisite/
  dependent endpoints and the relevant starting task revision for dependency.
  The dependency edge type must be `blocks`; no reversed endpoint inference.
- Snapshot = scope, captured_cursor, sorted selected current projections,
  snapshot_digest. Its digest covers the canonical typed scope/cursor/projections,
  excluding only the digest field itself. Absent/deleted/retired are explicit.
- CommitGroup = scope, committed_sequence, group_id, sorted selected final facts.
  Node facts contain before/after projections, mutation_count and transition_class
  (`created`, `lifecycle_only`, `record_changed`, `multiple_changes`, `deleted`,
  `domain_invariant_failure`). No transaction-internal state is published.
- RegistrationReceipt = subscriber_id, actor, request_digest, condition_digest,
  exact selectors, scope, original_expires_at, snapshot, initial_cursor, receipt
  reference. Reference digest covers canonical receipt content excluding that
  reference, with version1 and kind `graph_observation_subscription`.
- Pull = contract, original receipt reference, scope, after, through, committed
  groups, next_cursor nullable, retained_floor, status (`available`, `gap`,
  `expired`, `released`, `source_changed`), gap nullable. A non-available status
  cannot contain a successful transition proof or silently advance next_cursor.

A no-after GET on `/observations/subscriptions/{id}/changes` is the bounded
original-receipt lookup for uncertain install; it returns the original snapshot,
not a replacement boundary. A404 never proves a concurrent earlier install cannot
commit: retry the identical subscriber/payload and wait for its actual receipt.
These receipt/lookup operations are source control, not ordinary playbook commands.

## Revision and transition evidence semantics

The Graph-owned source profile defines a starting node logical revision as the
canonical decimal representation of Graph content_revision, with an explicit
revision-kind field in the source receipt. AQ's opaque Condition.Task.revision
must be produced/checked by this adapter, not guessed from a CAS version or parsed
under another backend profile. Edge revision is the exact Graph edge version,
with edge ID/type/endpoints separately bound. Existing Node/Edge DTOs stay intact.

Node projections contain only actual identity/kind, CAS version, logical revision,
task state, deleted/retired flags and typed invariant status. Edge projections
contain identity/type, endpoints, source/target revision pins, actual edge version
and deletion state. New fields are body-free observation DTOs, not replacements
for normal records or a second editable graph state.

A terminal transition proof names the exact starting logical revision and actual
coalesced committed before/after snapshots, after revision, commit-group identity
and source cursor. A changed starting pin is revision_changed, not implicit
resubscription. Slice1 accepts a journal terminal transition only when its before logical
revision is exactly the pinned starting revision, after logical/CAS revisions
are each before+1, mutation_count is1, and transition_class is lifecycle_only.
That class compares actual before/after fields: task_state may change, and the
existing automatic verification/summary invalidations are allowed; arbitrary
content/relationship/retirement/deletion edits are not. Everything else is a
revision_changed/ambiguous transition, never automatic rebinding. This intentionally
conservative rule can be expanded only as an explicit source-profile change.
Already-terminal registration requires exact current logical revision and is a
separate current-snapshot case.
Successful versus settled is evaluated by AQ's closed condition policy against
completed/canceled source facts; Graph does not invent a failed execution state.

Across separate commits, active→completed followed by completed→active remains
recoverable evidence of the first committed terminal state, while a single
transaction's active→completed→active does not. A retained outcome is historical
and immutable. AQ26 must independently recheck current dependency eligibility
and source pins before admitting resumed work; this journal grants no execution
or authority to overlap writers. Dependency evidence binds edge identity/version,
its exact endpoints and the relevant terminal node transition/current state.

## Retention, gaps and failure behavior

The new journal is not old history. Start its first generation with a current
snapshot baseline and sequence0; do not backfill allocated history as committed
transitions. Requests for earlier provenance are unavailable/pre_generation.
Snapshots never prove an unobserved terminal interval before this boundary.

Retention maintenance uses the same workspace head lock and may advance only
across whole committed groups. Publish the retained
floor/coverage and configured byte/count/time bounds. If a requested cursor is
behind the floor, an anchor is missing, storage continuity changes, or a complete
group cannot be retained, return a typed gap identifying the lost range/reason.
Do not replace it with an empty success or a current snapshot that erases the
missed completed→reopened interval. Bound cleanup work; never block ordinary
Graph domain writes indefinitely behind a lagging observer. If a hard journal
budget cannot retain a complete group, commit a gap marker/coverage hold rather
than silently dropping that change or exceeding the storage quota. Exact quota
policy and SQL accounting need explicit owner tests in slice1.

AQ persists the stable wait/member/subscriber intent before remote install,
performs remote I/O outside its SQL transaction, then rechecks its current Graph
binding/policy before retaining the receipt. There is no atomic AQ+Graph commit.
On loss, repeat the same install/lookup; after restart pull from the retained
cursor and reobserve through the original boundary. Hints may be duplicated,
coalesced or lost. Gap, expiry, revoke and unsupported are explicit source states;
none renews a wait or turns absence of evidence into dependency completion.

## Qualification before any AQ wait adapter is enabled

1. PostgreSQL controlled barriers prove inverse allocation/commit ordering cannot
   skip a new journal group, rollback leaves no head/facts, same-workspace writers
   are ordered, and different workspaces proceed independently. Include stale
   READ COMMITTED/REPEATABLE READ/SERIALIZABLE snapshots and bounded retry/timeout.
2. All Mutation variants, batch, draft apply/validation rollback, restores,
   structural endpoint changes, parent moves, no-ops, CLI/HTTP/Store and ordinary
   SQL DML produce exactly the declared coalesced facts. No-op receipt replay adds
   no group. One-transaction complete/reopen does not satisfy a terminal wait;
   two commits do, including during registration/restart/response loss.
3. Registration vs writer, stable same-ID replay/conflict, credential rotation and
   revocation while a read is held, workspace/epoch/anchor tamper, missing/deleted/
   retired/revision-changed endpoints, full commit-group paging and byte caps.
4. Retention/gap/oversize/expiry, source generation reset, restart and restore
   procedures; no false fresh snapshot or import of old uncommitted ordering.
   Keep old schema/history/public contract goldens immutable.
5. Real PostgreSQL package/HTTP tests and hundred-agent central pull load: p50/
   p95/p99, writer head-lock time, receipt latency, WAL/bytes, cleanup work and
   query plans under old/fresh histories. Preserve existing mutation/read quotas
   and budgets; measure the write-serialization cost before rollout.

First code slice, after review: Graph-owned PostgreSQL journal+scope generation,
registration/snapshot and bounded read-only pull, closed fixtures/schema, targeted
concurrency/all-writer/gap tests and an explicit unsupported check against the
current4195 binary. No generic event bus, broker, ordinary playbook command, AQ
migration, worker copy or production promotion. AQ25 starts real Graph waits only
after that owner/API is independently qualified; its fake Graph foundation and
concrete Mailbox/Timer work can proceed in parallel. WRK16 needs no Graph API or
predicate logic. Any later canonical Graph schema/binary change has a separate
backup/migration/capability/rollback gate.

## Proposed first-slice implementation ownership

- Graph: next free native migration; journal/head/guard trigger functions; source
  generation/retention maintenance; new standalone observation domain/store/HTTP
  module and optional CLI inspection. Existing node/history DTOs and all normal
  mutation entry points remain frozen. The migration must be tested with direct
  DML, not just an AQ-only delivery fixture.
- AQ25: durable wait registration intent, remote subscribe/lookup/replay adapter,
  exact Graph source-profile decoder, condition evaluation and immutable outcome
  reference. Keep unavailable on current4195. No graph data mirror or polling
  fallback which loses transient committed states.
- AQ26: current eligibility/admission and fresh writer grant after an outcome.
- WRK16: verified continuation/parking/stop/material bootstrap, never a Graph
  predicate or event consumer. Early Graph outcomes remain centrally retained
  while parking/checkpoint work completes.

Review the API/transition/retention/lock-order decisions before code. The journal
is an authoritative append record of Graph changes, not a second editable task
database; normal graph state remains authoritative. The no-build audit supplies
no performance, concurrency, migration or deployment qualification yet.
