# Observation expiry review correction

Status: independently reviewed source correction433d728, now component-qualified
in [separate correction evidence](evidence/committed-observation-expiry/README.md).
The coordinated offline compile,25-case actual/pure regression group and strict
checks passed. Historical qualification remains immutable under
`evidence/committed-observation/`; its original passing tests did not cover this race.
No canonical4195/service/schema/data change or source advertisement is included.

The reviewer identified that HTTP sampled UTC once before registration/pull. The
Store reused that scalar across credential/head waits and all RR retries. A live
request could cross its original deadline while blocked, then install a control
or return an Available replay using stale arrival time. The earlier supplied-scalar
expiry vector did not establish freshness after a real lock wait.

The correction supplies a private trusted owner clock. Production HTTP always uses
SystemClock; fixed/advancing clocks exist only in component tests. No request field
can supply the eligibility clock. Registration samples again after current-credential
and head acquisition on each retry, checks the immutable original expiry before
inserting a new control, and fences its final response after commit. Existing-receipt
retry has the same final fence. Pull and lookup check after auth/read waits and use
one final postcommit response fence; if Available has expired, they return Expired
with no groups or next cursor. All paths preserve original receipt/snapshot/pins,
absolute expiry and retry-retention deadline. There is no renewal or silent restart.

The final owner decision linearizes at the postcommit clock check, with no further
await before returning the DTO. A commit may itself cross the deadline after a live
preinsert check; its original receipt remains durable but is returned Expired, never
Available or extended. This does not promise a response is delivered over a network
before its deadline. AQ must independently honor its original wait deadline; neither
network delay nor a retained receipt extends it. Source-changed/gap/released/error
statuses remain unavailable and never become completion evidence.

Three new actual-PostgreSQL tests passed in the correction's25-case observation group:

- A real backend blocks registration on a held head while the original lease is live;
  the actual owner UTC deadline passes before release. The response must be Expired
  with no installed control. Both unchanged-head wait and a physical-head update
  forcing an RR retry are covered. The waiter is proved using pg_blocking_pids,
  not sleep-only scheduling.
- A valid retained subscription has a real committed completion. A credential row
  lock blocks replay across the actual UTC expiry; it must return Expired, no event
  groups/cursor, and the exact original receipt and expiry.
- A deterministic advancing test clock crosses expiry only at the final postcommit
  decision. Replay must discard its prepared groups; a newly committed control must
  retain its exact expired receipt and original expiry/retention, never Available.

Generated DTO schema/examples and original domain timestamps remain unchanged.
An explicit SAVEPOINT runtime vector remains deferred: transactional source reasoning
covers it, but previous actual tests establish full rollback/preview only. No claim
of runtime-qualified savepoint, maximum capacity, restart/restore cutover, AQ25
admission or execution authority is added by this source followup.
