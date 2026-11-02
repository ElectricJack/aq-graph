# E2 Graph Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish v0.1.0 with an immutable GHCR image reference and provenance, and prove the schema 7 to 8 upgrade preserves durable data.

**Architecture:** Keep schema/API unchanged. Run real embedded migration SQL in a private PostgreSQL schema and roll back the entire fixture. Generate machine-readable release evidence from Buildx's pushed digest and the checked-out source, then attach it to the GitHub release. Pin both Docker base indexes.

**Tech Stack:** Rust 1.96.1, sqlx/PostgreSQL 17, Python standard library, Docker Buildx, GitHub Actions/GHCR.

**Spec:** AQ task smart-falcon-16.5 (E2); task contract `aq-server/docs/superpowers/plans/2026-11-02-v2-full-application-tasks.yaml`, E2.

## Global Constraints

- Each repository builds and tests alone; no sibling path dependencies, submodules, or floating branches in release inputs. Use the committed lockfile with `--locked`.
- No graph schema/API changes or second task lifecycle; AQ owns execution.
- No secrets in code, manifests, evidence or logs.
- Do not edit `docs/specs/*` or sibling architecture documents.
- Required checks: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, and cargo tests under available resource controls. Named acceptance test must run and pass, not match zero tests.
- Record exact commands, environment, source commit and limits in `docs/implementation/E2.md`; publish through guarded AQ branch push and integration policy.
- E2 is direct integration; no adversarial review is specified.

### Task 1: Schema upgrade regression

**Files:** Create `tests/migration_upgrade.rs`. Read `tests/migration_shared.rs`, `tests/ready_projection.rs` and migrations 0001–0008.

**Interfaces:** Consumes `AQ_GRAPH_TEST_DATABASE_URL`; produces `upgrade_from_previous_schema_preserves_data`.

- [x] Apply migrations 0001–0007 in a unique schema inside one transaction with a bounded statement timeout. Insert two workspaces, task states/tombstones/retirement, knowledge, live/deleted structural edges, prerequisites, informational edges, immutable node/edge revisions, link sets, changesets, history and receipts.
- [x] Capture all durable tables with deterministic ordered full-row JSON, including generated search data, before migration 0008. Expected regression assertions:
  ```rust
  assert_eq!(after, before, "upgrade must preserve every durable row");
  assert_eq!(projection, expected_projection);
  assert_eq!(ready_candidates, expected_ready_candidates);
  ```
- [x] Run the named test temporarily without the 0008 application; expect failure because the projection does not exist. Restore the real migration call and rerun; expect one passing named test.
- [x] Assert transaction rollback removes the private schema. Run related migration/projection tests; do not change migration SQL to fit the fixture.

### Task 2: Immutable release identity and provenance

**Files:** Create `scripts/release-evidence.py`, `scripts/test_release_evidence.py`; modify `.github/workflows/release.yml`, `Dockerfile`.

**Interfaces:** `write_evidence(root: Path, output: Path, image: str, digest: str, tag: str, revision: str) -> dict` consumes Buildx digest and checked-out Cargo/Docker inputs; produces `release-provenance.json` and `image-reference.txt`.

- [x] Write Python unit tests using temporary Cargo.toml/Cargo.lock/Dockerfile fixtures. Assert tag, digest reference, revision, toolchain, lock SHA-256 and both pinned bases. Reject mismatched package/tag versions, malformed image digest/source revision and unpinned Docker bases. Run `python3 -m unittest discover -s scripts -p 'test_release_evidence.py'`; expect missing implementation failure.
- [x] Resolve Docker Hub multi-platform index digests using `docker buildx imagetools inspect rust:1.96.1-bookworm` and `debian:bookworm-slim`. Pin both `FROM` inputs as `name:tag@sha256:<registry digest>`.
- [x] Implement the generator with standard-library parsing/hash validation. Example identity construction:
  ```python
  reference = f'{image}@{digest}'
  lockfile_sha256 = hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest()
  ```
- [x] Add an id to `docker/build-push-action`, explicit linux/amd64 platform, BuildKit provenance and source revision label. After pushing, run the generator with `steps.build.outputs.digest`; upload records as workflow artifacts and attach them with binary archives to the GitHub release. Publish the digest reference and provenance in the job summary/release notes. Keep version guard and required checks.
- [x] Rerun Python tests and validate workflow YAML plus embedded shell/Python where available.

### Task 3: Operations, verification and release

**Files:** Modify `docs/operations.md`, `docs/implementation-status.md`, `docs/deployment-validation.md`; create `docs/implementation/E2.md` and retain release provenance in `docs/implementation/artifacts/`.

- [x] Document schema 8; forbid any old image on a newer schema. State migrations 0006/0007/0008 have no supported reverse migration. Rollback restores a verified pre-upgrade backup and reconciles later writes; derived projection repair is not downgrade.
- [x] Run formatter, Clippy, the named acceptance command and related migration/projection tests with capped jobs/threads. Run release-evidence unit tests. Record exact outcomes, including unavailable checks; do not claim an unrun full suite.
- [x] Self-review spec coverage; record no adversarial review requirement. Reconcile all E2 items in the evidence document.
- [x] Squash local implementation history to one leaf commit, merge the authorized integration branch, rerun targeted tests if merge changes touched files, guarded push, record full SHA. Obtain supervisor guidance for missing origin/dev and tag publication before performing dependent operations.
- [ ] Create immutable v0.1.0 at the approved pushed source. Verify release workflow/checks, retrieve release JSON/reference and compare GHCR registry digest. Record tag, source, workflow URL, digest and provenance in E2 evidence and implementation status; guarded-push evidence update.
- [x] Settle AQ checklist, close pass only if acceptance is met, and drain-ack. Publication cannot complete: record the exact blocker and close fail/blocked as directed by the supervisor, rather than inventing a digest.

Execution status: implementation and local checks complete. Supervisor instruction `msg-780101497caf4fb185a08fdf46469ffe` forbids tag/GHCR publication pending owner approval; release acceptance remains unmet and the task must close with that limit.
