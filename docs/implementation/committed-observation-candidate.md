# Committed observation candidate: Graph 0.1.1 / migration 0009

Status: isolated source candidate. No compilation, PostgreSQL execution, service
upgrade, canonical capability, capacity claim or AQ wait admission is qualified by
this document. The approved proposal is `committed-observation-proposal.md`; this
records the concrete first implementation and handshake refinements. The existing
4195 service remains unsupported for this new source protocol. AQ SQL38 is not
allocated or used. Existing Graph node/edge/history formats and routes are intact.

## Exact starting boundary and registration handshake

New authenticated workspace-relative routes:

1. `POST observations/snapshot`: closed `SnapshotRequest` names at most16 unpinned
   task or dependency targets. The owner authenticates the exact current token,
   takes the workspace head write lock and performs **plain MVCC** node/edge reads
   inside the same repeatable-read transaction. It returns actual logical-record
   revisions, lifecycle/identity/endpoints, an object snapshot digest, and the
   corresponding committed cursor together. Reads never take node/edge row locks
   after head. Missing objects are explicit nullable projections, not empty success.
   No per-request source row is persisted.
2. AQ validates actual source identity/binding/profile and closed bytes, then
   durably retains this snapshot, its digest, exact starting logical pins, cursorC,
   wait/member generation, and immutable condition digest **before** remote install.
   `content_revision` is the existing Graph logical-record revision encoded as a
   canonical positive decimal string; it includes lifecycle/structural changes.
   Independently reading a node followed by a later cursor is insufficient. No
   inferred content-only revision or CAS-only reinterpretation is allowed.
3. `POST observations/subscriptions` uses stable subscriber UUID, condition digest,
   original absolute expiry, exact selectors/pins, expected stream/generation, and
   optional `from_cursor=C`. Its immutable receipt includes original request digest,
   selectors, initial replay cursorC and a second snapshot at installation headH.
   A retained exact retry returns the original receipt; changing bytes for the same
   actor/subscriber refuses. The second snapshot does not overwrite the starting
   pin or consumeC→H. The owner does not claim to validate a client-retained snapshot
   merely because the request supplies its cursor; AQ must retain/validate the real
   owner response. A cursor identifies a boundary, not authority or a signature.
4. On response loss, GET `observations/subscriptions/{id}/changes?request_digest=...`
   without `after` is **lookup_only**, returns original receipt and no groups or
   next cursor. Its `through` remains originalC. Missing retained receipt is410;
   it never installs a replacement boundary. After pruning, an expired original
   registration cannot reinstall. AQ must not mint a new expiry or subscriber to
   conceal a lost receipt.
5. GET `.../changes?after=<canonical cursor>&receipt_digest=...` reads bounded complete
   commitgroups. `through` can pin the original page's head across subsequent pages;
   no new commits beyond it are mixed into that replay cycle. Pages contain at most16
   groups. Groups are never split; no-progress oversized groups refuse. Empty success
   advances only to the proven head/explicit through boundary. Released/expired,
   source_changed, gap and missing-receipt responses cannot be treated as empty replay.
6. DELETE `observations/subscriptions/{id}?receipt_digest=...` is exact-receipt,
   idempotent release. Released records remain counted through the retention horizon.
   A new graph capability route publishes only this profile's actual source head,
   scope, coverage and finite limits, with authority `none`.

The starting snapshot and cursor are usable only together under the actual owner
profile. No-cursor registration begins at installationH; stale starting pins must
be classified changed/unavailable by AQ, not rebound. Tasks/dependencies, edge type,
endpoint direction, source service/project binding, condition policy and exact pins
must be validated by AQ before accepting a source fact. A snapshot or event alone
cannot create a wake or grant. A resolved historical reference remains immutable;
new work admission must recheck current dependency eligibility and exact pins.

## Committed journal and conservative lifecycle proof

Migration0009 installs workspace heads and triggers while excluding ordinary
workspace/node/edge writers. The first actual DML change per workspace/SQL
transaction allocates sequence+UUID under a transactional head lock retained through
commit. Rollback rolls back the head, journal and domain together. TransactionID
only coalesces in-flight work; it is never an ordering cursor. Every row DML writer,
including Store/API/CLI, ordinary SQL/COPY, drafts applied, restores and structural
endpoint changes runs the same triggers. Validation previews roll back. Draft
storage, auth, derived ready projection and no-op updates do not advance the stream.

The fact stores first before/final after per object in that transaction, mutation
count and conservative transition class. A single normal lifecycle update may clear
summary and reset verification as existing Graph code does; those automatic
invalidations do not disqualify it. Other durable changes, structural checkpointing
or more than one object update disqualify a direct pinned terminal transition. The
helper requires exact object identity, initial logical revision and before/after
CAS+logical revision increments1, live/unretired task and nonterminal→terminal.
Same-transaction complete→reopen produces one multiple_changes fact and never a
terminal proof. Separate committed completion→reopen retains both groups; historical
completion is distinguishable from current eligibility. Arbitrary direct SQL that
breaks monotonic revision rules holds coverage with explicit domain_invariant gap,
while its domain commit remains visible. No historical API row is promoted to proof.

Node/edge identity/workspace moves and TRUNCATE are refused. Privileged trigger
bypass/replication-role bypass, DDL replacement and uncoordinated physical restore
are outside the ordinary-writer fault model. Trigger-owning DBA is trusted. A
restored source MUST rotate generation before publishing this profile; restore
cannot be detected using cloned in-database state alone. Offline owners must revoke
all source support until that step completes. `graph_observation_reset(workspace,
expected_generation)` is an operator-only SQL function, revoked from PUBLIC, with
no HTTP route. It atomically clears journal and changes generation. It preserves
bounded original receipt rows so old callers see source_changed, not replacement.
A restored/cloned source with ambiguous lineage must rotate stream UUID as well via
reviewed maintenance; there is no automatic safe cutover in this candidate.

## Frozen bounds and overload behavior

All byte limits below are **logical retained JSON/accounting bounds**, not a claim
about PostgreSQL relation/index/WAL/TOAST size, free space or autovacuum. Physical
capacity requires separate measured operational evidence. Domain Graph quotas are
unchanged; the journal does not promise retention duration or agent capacity.

| Resource | First candidate bound |
| --- | --- |
| targets/selectors |16; at most48 distinct object projections |
| request / response |65536 /262144 bytes; existing transport envelope stays unchanged |
| groups/page |1..16; caller bytes65536..262144; whole groups only |
| journal/workspace |16777216 logical bytes;32768 batches;65536 facts |
| fact / batch |4096 logical bytes/fact;4096 facts/transaction/workspace |
| accounting |256 bytes/batch +128 bytes/fact +before/after SQL JSON text bytes |
| retained subscriptions/workspace |1024 rows and16777216 logical bytes |
| retained subscriptions/actor |256 rows; actor namespace remains credential-bound |
| subscription row / receipt |131072 /65536 logical bytes |
| original lease |at most3600000ms from first install; exact retries cannot extend |
| retry horizon |original expiry+3600000ms, including active/expired/released records |
| expiry cleanup |at most32 rows per registration, under same head; no background service |

Workspace head is preallocated metadata outside the journal allocation budget. On
any journal/fact/transaction overflow the current transaction marks coveragegap
with earliest lost sequence/reason; any partial batch is marked gap and never served
as complete. All later domain writes continue, updating only the fixed head per
transaction; they allocate no more journal/gap rows until operator maintenance.
No lagging subscription can pin unlimited retention or block a writer. The current
slice retains complete journal until the fixed cap, then **holds explicitly gapped**;
it does not pretend to have a continuously compacting retained window. Reset rotates
generation; no implicit resubscribe or current-snapshot fallback crosses a gap.

Control capacity counts all active/expired/released receipts and tombstones. Bounded
expiry cleanup commits even when a fresh registration remains overloaded. No new
control is created when rows/bytes/actor limits refuse. Receipt loss returns410;
source/generation change is explicit. Original absolute expiry is immutable. Register
and starting snapshot retry40001/40P01 at most8 times with bounded exponential delay;
all operations have existing4s statement/2s lock/10s HTTP envelopes. Resource timeout
or exhausted retry maps to503. Plain SQL callers must retry their **entire** domain
transaction after deadlock/serialization failure; there is no invisible partial retry.
Multiple-workspace transactions may deadlock if writers acquire heads in opposite
orders; that is bounded/refused, never solved by remote work under a head lock.

## Source tests prepared; actual qualification still required

The candidate has pure closed-wire/cursor/digest/terminal-helper tests and actual
PostgreSQL tests for atomic snapshot/registration replay, complete→reopen in one vs
separate commits, normal completion clearing a summary, fixed-through pagination,
rollback/no-op/preview, commit-head serialization, snapshot/node lock inversion,
edge/ordinary-SQL coverage, identity/TRUNCATE refusal, explicit capacity gap with
continued writes, invalid-writer gap, reset/original expiry/release/current-token
revocation, retained actor quota/32-row expiry cleanup, actual HTTP auth/410, and a
small measured write/snapshot latency smoke. Generated schema/examples are produced
only from the typed candidate via `examples/observation_contract.rs`; their golden
bytes/hashes will be frozen after compilation. Existing tests/goldens must remain
unchanged. Additional raw interleavings and quota checks may refine tests before
qualification. No test result is claimed here yet.

Request a bounded2-target offline build/test lease and disposable Graph-only
PostgreSQL database before execution. Record exact source/schema/binary/DB lineage,
counts, timings and remaining limits. No canonical service/schema/data update or AQ
production source eligibility is authorized by this source candidate.
