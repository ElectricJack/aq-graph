# E2 Graph Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish the first release with an immutable GHCR image reference and provenance, and prove the schema 7 to 8 upgrade preserves durable data.

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
- [x] Create the immutable release at the approved pushed source (v0.1.1 after the Task 4 repair; preserve v0.1.0). Verify release workflow/checks, retrieve release JSON/reference and compare GHCR registry digest. Record tag, source, workflow URL, digest and provenance in E2 evidence and implementation status; guarded-push evidence update.
- [x] Settle AQ checklist and prepare passing close only after every acceptance item is met; follow the close next-claim result. Record actual release identity and verification before closing.

Execution status: implementation and local checks complete. The owner authorized agents to create release tags and publish images in-task on 2026-11-02 (recovery comment on `smart-falcon-16.5`), superseding the earlier publication hold. Tag `v0.1.0` is preserved at `775cbbe3cfa2e419069262833d6ccfac8427b2f4`; its publication failed before GHCR due to artifact selection. The corrective `v0.1.1` release at `2d5b02c610b53baec03658171e1d6e6ebece4bd2` passed workflow `41558682185`. Actual registry digest, assets/provenance and checks are reconciled in `docs/implementation/E2.md`.


### Task 4: Repair archive selection and publish v0.1.1

**Files:** Modify `.github/workflows/release.yml:69-70`, `Cargo.toml:3` and the local `aq-graph` package entry in `Cargo.lock`, and `Dockerfile:18` (the default version build argument). Update `docs/implementation/E2.md`, `docs/implementation-status.md` and retained artifacts after verifying publication.

**Interfaces:** Consumes `aq-graph-linux-x86_64` and `aq-graph-macos-aarch64` upload artifacts; excludes Buildx `.dockerbuild` records. Produces tag `v0.1.1`, its GHCR digest and release provenance with matching source/lockfile identity. Preserve `v0.1.0` at its existing commit.

- [x] Regression reproduced by release run `41557588416`: both archives downloaded successfully; the third artifact `ElectricJack~aq-graph~S9AJS7.dockerbuild` failed extraction. No image/release was published. Retain the relevant job log lines as the red check.
- [x] Select only release archive artifacts:
  ```yaml
  with:
    pattern: aq-graph-*
    path: artifacts
    merge-multiple: true
  ```
  This matches the two binary uploads, excludes the build record, and runs before this job uploads its own release evidence.
- [x] Set `[package].version` to `0.1.1` in `Cargo.toml` and the root package version to `0.1.1` in `Cargo.lock`; keep all dependency versions and checksums identical. Set `Dockerfile` default `ARG AQ_GRAPH_VERSION=0.1.1` to match the package. Do not move/delete the existing tag.
- [x] Run `/tmp/e2-actionlint/actionlint .github/workflows/release.yml .github/workflows/ci.yml`, `cargo fmt --check`, capped `cargo clippy --locked --all-targets -- -D warnings`, the exact named upgrade test against disposable PostgreSQL, and `aq test scripts/test_release_evidence.py scripts/test_benchmark.py`. Expected: no diagnostics; exactly one passing named upgrade test; 12 Python tests pass. Actual tag CI must also pass the full existing checks/deployment drill and both binary builds.
- [x] Commit the repair, merge approved `origin/main`, guarded-push and create `v0.1.1` at that pushed commit using `gh api --method POST repos/ElectricJack/aq-graph/git/refs -f ref=refs/tags/v0.1.1 -f sha=2d5b02c610b53baec03658171e1d6e6ebece4bd2`. Watch that one release run in the foreground.
- [x] Download the actual release `image-reference.txt`, `release-provenance.json` and `Cargo.lock`; compare source, lock digest, schema/API/toolchain and registry tag/index digest, and inspect the linux/amd64 manifest and provenance. Record the exact real identity and workflow result in E2 evidence/status, guarded-push the evidence commit, then close truthfully.

Self-review: this repair addresses the observed publication failure within E2, retains immutable history and all required CI, changes no API/schema, and follows the task's explicit allowance to bump the package version. No independent subsystem or adversarial review is added.
