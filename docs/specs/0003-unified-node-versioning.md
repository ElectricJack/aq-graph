# Shared version control for task and knowledge nodes

Task and knowledge nodes use one revision engine. Kind-specific fields, edit permissions, graph invariants, and verification policy remain validation rules rather than separate version-control mechanisms. This specification supersedes earlier statements that task lifecycle and ordinary link changes do not create node revisions.

## Revision boundary

Every meaningful durable change advances the node's optimistic `version` and its immutable `content_revision`. The latter retains its API name for compatibility but now identifies a complete durable node revision: content, lifecycle, and the recorded relationship membership. No-op operations and exact idempotent retries create no revision. Server timestamps use PostgreSQL microsecond precision so mutation responses, current reads, and historical snapshots agree. Changes within an atomic batch remain ordered and share a change-set identity.

Recorded relationships include all outgoing edges and incoming structural edges (`blocks` and `parent-child`). This includes a task's prerequisites and parent membership. Structural changes advance both endpoints; informational changes advance the source. Incoming informational links are derived from other records and are not owned by the target's revision. Changing a linked target's content does not revise the referring node. Pinned and floating target references retain their explicit meanings; a node revision does not freeze the entire graph.

Immutable relationship versions are stored separately. Node revisions reference immutable relationship sets, and unchanged sets are reused for content or lifecycle changes. Live queries continue to read indexed current-state tables. Heartbeats, worker claims, caches, and other transient execution data remain outside graph revisions.

## Read, diff, and restore

The same endpoints and commands apply to both kinds. Exact revision reads return the historical node and its recorded relationship versions. Diffs include fields and links. Restoration is an atomic, expected-version-checked operation that creates a new revision without removing intervening history. Removed links regain their stable identities with new edge versions; links absent from the requested revision are removed.

By default restoration preserves the current lifecycle: task state, retired state/reason, and deletion state. `include_lifecycle: true` explicitly restores those historical values too. This default applies equally to task and knowledge nodes. A deleted node cannot regain active relationships while remaining deleted; that restoration requires an explicit lifecycle restore to a live revision.

Restoration rechecks current workspace access, permissions, endpoint availability, revision pins, single-parent/depth/cycle rules, and completion guards. A previously valid revision can be incompatible with today's graph; refusal rolls back the entire operation. Restore does not bypass authority checks or reinstate historical verification badges. A meaningful knowledge correction resets verification; a true no-op preserves current verification. At most 100 relationship changes may be restored in one operation.

## Existing history

Migration 0006 leaves existing snapshots and history payloads unchanged. Their exact relationship membership was not recorded, so it must not be guessed from current edges. Exact reads expose `relationships_available: false` with `links: null` for such revisions, and full restore refuses them. Migration adds an explicitly attributed current-state baseline for each existing node, including current relationship membership. Subsequent revisions are complete under this contract.

## Verification

Run the same behavioral scenarios for both kinds: creation, content and lifecycle revisions, links, exact reads, diffs, restoration, no-ops, CAS, replay, workspace isolation, and rollback. Exercise structural restoration against concurrent graph constraints and verify migrations preserve old history. Existing tests that asserted intentionally superseded semantics must be updated explicitly; source compatibility maps must retain those divergences. Measure current-query behavior separately from revision-write cost; prior benchmark results do not establish performance of this revision engine.
