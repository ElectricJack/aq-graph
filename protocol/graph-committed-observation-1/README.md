# graph-committed-observation-1

Additive Graph0.1.1 candidate profile. `schema.json` is generated from closed Rust
wire DTOs; its raw-HTTP `anyOf` family does not add an artificial transport envelope.
Revision/cursor/digest, lease, cardinality and byte semantics additionally require
the runtime validation and owning transaction rules in the implementation spec.

The three request/response examples are **synthetic typed contract examples**, not
an actual graph, persisted commit, source receipt, completion or execution proof.
UUIDs/revisions/cursors may be accepted only from a qualified actual owner. Existing
node/edge/history DTOs and their cursor semantics remain unchanged.

Regenerate intentionally with:

```
cargo run --offline --locked --example observation_contract -- protocol/graph-committed-observation-1
```

See `docs/implementation/committed-observation-candidate.md` and its qualification
evidence. The service currently at4195 does not expose this new owner profile.
