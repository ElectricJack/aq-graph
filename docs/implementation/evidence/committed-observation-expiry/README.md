# Observation expiry correction qualification

Date: 2026-11-01. Independently reviewed product source
`433d7282eb524edb63a6f4da266ebed816ecbc68`, aq-graph 0.1.1 / schema 0009. The original
[5e3595f qualification](../committed-observation/README.md) and its failure logs
remain unchanged. This checkpoint closes the stale-arrival-clock review blocker
for the isolated component; it does not deploy or advertise a production source.

The [review correction](../../committed-observation-expiry-review.md) describes the
fresh trusted owner clock, immutable original expiry and final postcommit response
linearization. [qualification.json](qualification.json) records exact source,
binary/profile hashes, test composition, actual database cleanup and limits.

| Check | Actual result |
| --- | --- |
| Offline/locked lib-test compile, jobs 2 / debug 0 / incremental 0 |PASS 10.47 s |
| Observation source group, threads 2 |25 passed, 0 failed/ignored, 14 old library tests intentionally filtered, 5.30 s |
| New expiry cases included in that group |3 passed; head case exercises both unchanged-head wait and forced RR retry |
| Affected all-target Clippy, warnings denied |PASS 6.60 s |
| Generated schema/examples |byte-for-byte unchanged; existing frozen-schema/digest vectors pass |
| Exact-owned actual Graph database |fresh UUID database on existing test55432; positive removal verified |

The 25-case group comprises the 22 preceding observation tests plus 3 new actual
PostgreSQL expiry tests (4 pure and 21 PostgreSQL cases total). The old full 147-test
checkpoint is historical; this correction does not claim those 147 tests were
rerun. All-target strict compiles the affected existing test/CLI/example surfaces.
No source fixes, new failures or ignored tests occurred during this correction's
scheduled checks. SERVER was released directly to Runner immediately afterwards.

The new cases prove:

- The actual registration backend is blocked on a held workspace head while its
  original lease is live. Actual owner UTC crosses expiry before lock release;
  registration returns Expired without a fresh control. The second variant commits
  a physical head-row update so the stale RR transaction must retry before the same
  expired result. pg_blocking_pids establishes a real database barrier, not just
  sleep-based scheduling.
- A real retained receipt and committed completion exist before a credential row
  lock blocks replay. After the original UTC deadline passes, replay returns
  Expired with no groups or next cursor, preserving the exact original receipt and
  expiry. Access authority and clock freshness are both checked after the wait.
- A deterministic advancing **test-only** clock crosses expiry only at the final
  postcommit decision. The prepared replay proof is cleared. A control whose commit
  crosses its deadline retains its exact expired receipt/expiry/retry horizon and
  cannot return Available or acquire a fresh lease. This case is not labelled a
  physical wall-clock measurement.

The final test binary SHA256 is
`39829bc7704400a6b8b1d9c9354cb776ce14c5db94b44804a3cdf50d32a4d853`.
Schema SHA256 remains
`c77a4b72eec1f404f4a2a9b433f447ad3fca2192c434787180bc6d0459a3f138`.
The existing exact TestDb guard binary was reused and verified unchanged. Only its
fresh Graph UUID database was created; read-only post-run verification confirmed
its absence on PostgreSQL 17.11 / system ID 7702509189573074990. No canonical 4195
process/schema/data, user main-checkout files, new service/container or AQ SQL
migration changed.

The incidental 24-write smoke measured write p50/p95 of 9.825/12.106 ms and atomic snapshot
3.747/4.271 ms under two-test fixture load. This remains a small local sample, not a
production capacity, throughput delta or maximum-retained-history measurement.

Availability linearizes at the final owner clock decision after commit, without a
further await before returning the DTO; network delivery can occur later. AQ must
honor its original wait deadline independently. SAVEPOINT runtime qualification,
maximum/sparse journal performance, physical restart/restore cutover, AQ25 source
admission and execution authority remain open. The current 4195 backend remains
unsupported for this new profile until separately authorized deployment qualification.
