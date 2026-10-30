# v1 behavioral compatibility

**Scope update:** AQ v2 is a fresh start. This document records selected adopted behaviors and their test evidence, not a commitment to implement every v1 feature. V1 import/history migration is excluded from aq-graph; any optional owner-specific transfer belongs in an external script. Internal aq-graph schema upgrades are separate.

aq-graph will use selected, explicitly adopted v1 tests as executable behavioral contracts. Preserve their preconditions, invariants, and observable outcomes while adapting transport, identities, and authentication to the standalone API. Do not reproduce database mocks, command dispatch plumbing, or orchestration dependencies solely to make old test code run.

The source inventory is pinned to a v1 Git revision and records individual test names. A translated case must link to that source and state any intentional adaptation. An inventory entry is not coverage evidence. Passing new tests does not establish compatibility with unported cases or a production migration.

## Service boundary

Graph storage, hierarchy invariants, intrinsic readiness, knowledge ownership, evidence validation, and revision protection belong in aq-graph. Worker claims, scheduling, provider selection, execution gates, integration admission, and orchestration policy stay in Agent Q. External review decisions may later grant explicitly scoped authority through an adapter; aq-graph must not read AQ tables.

The first compatibility increment covers graph and knowledge invariants that can be expressed without AQ runtime state. It does not claim to finish the full source inventory. Remaining graph features, global sharing, artifact retention, authority grants, redaction, and richer context selection are assessed against v2 needs rather than automatically scheduled for parity. Historical v1 migration is outside the product scope.

## Graph contracts replacing the initial baseline

Parent-child relationships form a forest with one live parent per child and at most three structural levels. A parent in draft withholds its descendants from intrinsic readiness; containers with live children do not themselves appear as runnable leaves. A completed or canceled parent refuses new children. Completing a parent requires its children to be completed or canceled; automated container settlement still belongs to AQ because it must include execution gates.

Cycles across the combined `blocks` and `parent-child` topology are refused. Parent assignment is an atomic, version-checked operation supporting reparenting and detachment. A bounded tree read includes only structural children and reports truncation. Migration must refuse an existing graph that violates the new constraints rather than silently deleting relationships.

Informational link endpoint rules follow v1 records: `motivated_by` and `produces` connect task to knowledge; `supports`, `contradicts`, and `supersedes` connect knowledge to knowledge; `references` may connect either kind. These links do not alter task execution state or readiness. Typed dependency duplicates are no-ops, while duplicate informational links remain conflicts. Descriptions are retained on edges.

These rules supersede the initial specification's organizational-only parent-child behavior and separate cycle validation. Missing waits, contingency, and delivery edge semantics still require explicit adapter/state contracts; do not coerce them into `blocks`.

## Knowledge contracts replacing the initial baseline

Authenticated creation records an immutable author. Ordinary writers may revise their own unverified findings, while administrators may revise other authors' findings and protected revisions. Any meaningful edit of verified or disputed knowledge resets verification; historical verification cannot be carried forward to changed content.

Creation always starts unverified. A dedicated administrator verification operation requires named, typed evidence and a reason, records the authenticated actor and server time, and creates an immutable revision. Ordinary patching cannot set verification. Typed source descriptors validate their own required fields and identifiers. This does not prove the content of externally retained artifacts; authoritative artifact/review integration remains a separate contract.

Retirement requires administrator authority and a reason. Default search excludes retired and disputed knowledge, with explicit inclusion flags. Draft creation permits proposing a correction to another author's finding; applying knowledge proposals requires administrator authority and rechecks the current record. Preview remains rollback-only and cannot bypass authorization for committed writes.

Previously created records recover authorship from their initial revision. Historical snapshots stay immutable. Existing verified content lacking evidence attribution must not be silently blessed by migration; operators receive an actionable migration error. Restoring knowledge content creates a new unverified revision. Task restoration continues to preserve current task lifecycle.

## Validation and evidence

Run translated tests against the unchanged baseline first and retain their failure summary. Implement the matching contracts, then rerun the translated cases and existing regression suite against real PostgreSQL. Concurrency tests cover invariant preservation rather than merely sequential behavior. Do not mark missing capabilities as passing by skipping tests or weakening their expectations.

The source inventory distinguishes translated coverage, unverified existing coverage candidates, pending core work, and explicit AQ/provider/migration boundaries. Report partial assertions and identity/role adaptations accurately. Recheck query plans and performance when readiness or filtering changes invalidate earlier benchmark assumptions.
