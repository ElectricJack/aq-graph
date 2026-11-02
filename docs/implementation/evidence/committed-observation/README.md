# Isolated Graph committed-observation component qualification

Date2026-11-01. Product source `5e3595fe33ff823ab667576146de34a5520484cc`,
base `225329cd6acf5a775fd2a0b016dd1c45fb16c79b`; aq-graph0.1.1, reserved Graph
migration0009 and additive `graph-committed-observation-1`. This is component
qualification, **not deployment or AQ wait-source admission**. The existing4195
Graph process/schema/data were not changed. No AQ SQL38, new service or container
was used. Existing main-checkout user integration docs were not edited.

[qualification.json](qualification.json) records source, generated profile, binary
and guard hashes, individual run boundaries, failures, limits and latency samples.
The [candidate](../../committed-observation-candidate.md) freezes the exact protocol,
quotas, operator restore requirements and scope-deletion exception. The original
[proposal](../../committed-observation-proposal.md) remains separately identifiable.

## Actual environment and ownership

All Cargo operations used offline/locked, jobs2, debug0/incremental0 and the existing
coordinated `/tmp/aq-native-union-target`. Actual tests used threads2 and a fresh
UUID-named `aq_test_<UUIDsimple>` database on the existing test PostgreSQL55432
cluster, administrative database/user postgres. The qualification wrapper reuses
AQ's exact TestDb UUID create/options/Drop blocks; no AQ server/runner library is
compiled into Graph. It additionally checks host/port/admin database/user, UUID
name and positive exact-owned database removal. No canonical/4195 database or
retained rollback database was targeted. Every child database was removed.

The HTTP assertions exercise the actual production Axum router against actual
PostgreSQL via in-process HTTP requests. They do not claim an externally served
TCP deployment, process restart, browser acceptance or physical restore. Contract
example UUIDs are synthetic typed examples; no fixture timestamp or allocated old
history sequence is a committed watermark.

## Passing checks and retained failures

| Check | Actual result |
| --- | --- |
| Initial lib/test compile |PASS38.15s |
| Initial source group |20PASS/1FAIL,0ignored,3.68s; inherited workspace path rejection400 instead of intended410 |
| Corrected actual HTTP/source group |21PASS,0failed/ignored,3.87s |
| First full suite |stopped at legacy emptied-workspace cleanup FK refusal; failure retained |
| Full suite after source FK fix |147PASS across18 nonzero test binaries;0failed/ignored;2 zero-test example harnesses; summed test40.25s, compile11.02s |
| Additive schema8→9 upgrade |1PASS0.09s in full suite; original domain/history/ready bytes unchanged, new baseline0, first actual DML1 |
| Final source group, including added scope-deletion case |22PASS,0failed/ignored,14 old lib tests filtered,4.24s;4 pure+18 actual PostgreSQL cases |
| Final affected strict all-target Clippy |PASS6.43s |
| Schema/examples |generated from closed typed raw-HTTP DTOs; frozen-byte and digest assertions PASS |

The final full run preceded one **test-only** scope-deletion/recreation addition;
its production source is unchanged. The final22-case source run plus the full
147-case regression therefore cover148 distinct tests across the two runs; this
is not presented as a single148-test full-suite execution. Final Clippy includes
the added test. Existing Graph tests and public golden inputs were not edited.

Other retained initial failures: the new qualification example initially treated
SQLx `get_database()` as a string instead of Option; strict initially found a
collapsible if, manual range test and large schema-family enum. All were corrected;
failed compiler/strict/test logs remain beside the passing logs. The HTTP failure
was fixed by closing the path over both inherited workspace and subscription ID,
then verifying authenticated workspace agreement. The new private source FK
cascade preserves an existing legal workspace cleanup; no old test was weakened.

## What the actual cases establish

- A starting snapshot returns actual logical-record revisions and committed cursor
  in one head-locked/MVCC transaction; no node row lock is taken after head. A held
  node row lock does not block that snapshot within the test's1s bound.
- The competing real SQL backend is observed active with `wait_event_type=Lock`
  while the first writer owns the head. It cannot allocate/publish an overtaking
  commit. This uses a real backend lock barrier, not sleep-only scheduling.
- Separate committed completion→reopen remains replayable from the retained atomic
  starting cursor, including install after both commits. One SQL transaction doing
  both produces one coalesced multiple_changes fact and no terminal proof.
- Actual Store completion clears an existing summary and still qualifies the
  conservative pinned lifecycle transition. Arbitrary content changes and multiple
  updates do not acquire that classification.
- Fixed-through pagination, complete groups, original stable-ID receipt retry,
  changed-payload conflict, lookup-only response, expiry/release/reset/current-token
  rotation, actor/workspace isolation and no-cursor stale-pin non-rebinding pass.
- Ordinary edge/SQL writes, object-kind+UUID identity, rollback/no-op/preview,
  identity/TRUNCATE refusal and unsupported-revision gap behavior pass.
- A real4097-row domain INSERT commits all rows but retains only4096 source facts
  with a reserved explicit transaction-fact gap. Subsequent writes continue without
  growing journal rows while coverage is held. The journal-byte boundary uses an
  explicitly injected accounting-bound fixture; it is not a claim of naturally
  filling16MiB or a physical PostgreSQL disk measurement.
- Real256 actor subscriptions, including released receipts, reach the control quota;
  bounded32-row expiry cleanup makes room without losing the original lease rules.
- A retained subscription and journal are genuinely removed with an authoritatively
  deleted legal scope. Old credentials become unauthorized. Recreating the exact
  workspace UUID creates a NEW stream UUID; old receipt remains unavailable and
  old expected scope cannot install a fresh subscription. This exception is not
  disguised as ordinary release/retention pruning or successful empty replay.

## Small latency sample and remaining work

The full-run24-operation smoke measured Store writes p50/p95 **8.898/10.238ms** and
atomic starting snapshots **3.246/3.733ms** while two fixture tests could run. This
includes real owner transactions and is a small local smoke, not a baseline delta,
production SLA, maximum-history pull measurement or hundred-agent capacity proof.

Still open: independent source/API review; every adversarial/multi-workspace
serialization interleaving; natural maximum journal/control-byte and sparse-pull
performance; real process/restart/restore deployment; continuous compaction; AQ25's
production source adapter and receipt validation; AQ26's current eligibility/wake
admission. The operator must revoke source support across ambiguous restore and
rotate generation/stream as documented. No Graph event, cursor or receipt grants
execution authority. The current4195 backend remains unsupported until an exact
new-profile deployment is separately authorized and qualified.
