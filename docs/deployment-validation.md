# Deployment and recovery validation

The repeatable command is:

```sh
python3 scripts/deployment-smoke.py
```

This runs the same script as CI. Locally it builds the production Dockerfile, uses the Compose PostgreSQL 17 image, generates isolated credentials in a temporary directory, and creates only project `aq-graph-validation` resources on loopback port 18746. Existing containers, volumes or networks under that project cause refusal before mutation. A local project lock prevents concurrent script invocations. Compose routing environment variables are removed and all direct Compose calls use explicit manifest, directory and project arguments. Helper scripts inherit the isolated project and run in the temporary copy. Cleanup on normal completion or Python exception removes only the owned disposable project. Forced process termination can leave those resources for explicit inspection and cleanup; the next run refuses to adopt them. Optimized Python mode is refused so validation assertions cannot silently disappear.

CI builds `aq-graph:validation` with Buildx, restores/exports the GitHub Actions cache, and loads that image into the runner's Docker daemon. It then runs the drill with `AQ_GRAPH_SMOKE_SKIP_BUILD=1` and the same explicit image tag. This keeps all deployment/recovery assertions while avoiding another image build inside the drill. The deployment job runs independently of the Rust/PostgreSQL checks; workflow success and reusable release checks require both jobs to pass.

The Dockerfile builds locked dependencies from `Cargo.toml`/`Cargo.lock` before copying application source or migrations. It deletes this package's placeholder artifacts before the real build, retaining dependency artifacts in the cached layer. Source or migration edits therefore invalidate the application layer; manifest, lockfile or toolchain changes also invalidate dependencies. Release publishing can read the same image-cache scope. Cache hits accelerate compilation but never skip the recovery drill.

## Where the drill runs and what each result means

The workflows are checked in under `.github/workflows/`:

- [`check` job in `ci.yml`](../.github/workflows/ci.yml): formatting, clippy, the Rust/PostgreSQL tests and the script checks. It does not build or start the container image.
- [`deployment` job in `ci.yml`](../.github/workflows/ci.yml): builds `aq-graph:validation` with `CARGO_PROFILE=ci-image`, loads it into the runner and runs the isolated deployment and recovery drill (`scripts/deployment-smoke.py`).
- [`release.yml`](../.github/workflows/release.yml): runs only for `v[0-9]*` tags. Its `checks` job reuses `ci.yml` (both jobs above), then `binaries` builds the archives and `publish` pushes the release image to GHCR and creates the GitHub release.

Three different claims can be made about a revision, and each needs different evidence:

| Claim | Evidence | What it does not show |
| --- | --- | --- |
| The isolated recovery drill passed | A green `deployment` job in the `ci.yml` run for that commit. The job log shows the drill, and the drill's recorded artifact lists image identity, PostgreSQL version and table digests. The image is the throwaway `aq-graph:validation` and is never pushed. | That an image was published, or that any installation runs this revision. |
| An image was published | A `release.yml` run for a version tag whose `publish` job succeeded. The GitHub release carries `image-reference.txt` and `release-provenance.json` that bind the GHCR digest to the source revision. The release image is built with the `release` profile, not `ci-image`, so it is not the image the drill ran. | That the drill ran against these exact bytes, or that the image is deployed anywhere. |
| An installed revision is healthy | Observation of the installation itself: the running image digest matches the released `image-reference.txt`, `GET /health/ready` succeeds and the recorded schema version matches (see [operations](operations.md)). | Nothing in CI or release workflows proves this. No installation is touched by either workflow. |

A maintainer should therefore read a green `deployment` job as "this source builds a container that survives the isolated restart, recreation and restore drill", a successful `publish` job plus its release assets as "this digest was published for this tag", and only a check of the running installation as "this installation is healthy". A release tag is gated by the drill through `checks`, but a green drill on a branch or `main` commit says nothing about a tag that was never pushed.

## Findings addressed

- An internal-only graph network allowed container readiness to succeed but refused HTTP connections to its published host port on Docker Engine 29.3.1. The graph now also joins a frontend network. PostgreSQL remains solely on the internal database network without a host port.
- CI built `aq-graph:ci` while Compose selected `aq-graph:local`, making the tested image ambiguous and triggering another build. CI now loads the selected image before invoking the drill; the drill asserts that exact image tag.
- Initializer config always selected port 8741 even when Compose used a custom port. New configs now honor `AQ_GRAPH_PORT`. Existing credentials and config remain unchanged on rerun; missing UID/GID settings are repaired independently.
- Backup publication could overwrite a preexisting checksum path. The helper now refuses either an existing dump or checksum destination.

## Evidence and limits

The fixture creates both record kinds, a relationship pinned to a target revision, a derived summary and an unapplied draft. Exact public API responses and receipt results are compared after restart, same-schema recreation and database restore. All public tables are compared by row count and deterministic full-row digest before and after restore. A fresh write and its idempotent receipt survive restart of the restored service. Invalid checksum, invalid archive with a matching checksum, existing restore database and authoritative database name are rejected. No production cutover occurs.

The recorded artifact includes image identity, PostgreSQL version, public-table counts/digests and total drill duration. Counts/digests are equality evidence for this fixture, not a production-scale benchmark. Container recreation uses the same image/schema within each drill; cross-schema upgrades through the deployed stack and previous-image rollback remain unvalidated. TLS/ingress, off-host encrypted backup storage, WAL/PITR and PostgreSQL major upgrades remain operator work.

Recorded on 2026-10-30 with Docker Engine 29.3.1 / Compose 5.1.1 and PostgreSQL 17.11. The production Dockerfile cold build passed (`docker build -t aq-graph:validation .`), producing image `sha256:5497465739a0dc0202856c39d5086dde45b78f60be84dc0e9698681c24ccdfc6`. The isolated drill using that image passed twice (36.30 and 35.05 seconds); the second run's [machine-readable evidence](artifacts/deployment/2026-10-30-recovery.json) records the table equality digests. The command for those runs was `AQ_GRAPH_SMOKE_SKIP_BUILD=1 python3 scripts/deployment-smoke.py`, following the actual Docker build. The parent independently reran the final frozen script through the default build path (`python3 scripts/deployment-smoke.py`) and passed in **38.08 seconds**, including final permission, custom-port and checksum guards. Its [independent evidence](artifacts/deployment/2026-10-30-parent-recovery.json) is retained separately.

After the readiness/history optimizations, the parent rebuilt the actual Dockerfile and repeated the complete drill again. It passed in **80.49 seconds including compilation**, with image `sha256:50565e8595155f29b3ac4baacdc390eab1ce7646b2dde8a2448aa665b03a3780`; [optimized-build recovery evidence](artifacts/deployment/2026-10-30-optimized-recovery.json) records the result.

The schema-8 build and full drill passed in **82.13 seconds including compilation**. Its fixture adds an active parent/child pair and checks that only the leaf appears in ready results across restart and restore. Exact table equality includes `node_ready_projection`; the restored clone accepts a new write and replays it after restart. [Schema-8 recovery evidence](artifacts/deployment/2026-10-30-schema8-recovery.json) records image identity, PostgreSQL version and all table digests. Separately, the performance fixture was upgraded from schema 7 to schema 8 before its first application load; this is a native-service migration check, not a public deployment or previous-image rollback test.

The CI cache change was validated locally by building the real image, running `aq-graph --version`, and completing the unchanged recovery drill with `AQ_GRAPH_SMOKE_SKIP_BUILD=1` in 39.02 seconds. A separate temporary build context changed only a source comment: Docker reused the locked-dependency layer, recompiled only aq-graph, and completed in 18.43 seconds. These local checks validate cache invalidation and the prebuilt-image path; GitHub timing must be measured separately. The cache setup follows [Docker's GitHub Actions cache guidance](https://docs.docker.com/build/ci/github-actions/cache/).

## Schema 7 to 8 regression

`cargo test --locked --workspace upgrade_from_previous_schema_preserves_data` exercises migrations 0001–0007, inserts both node kinds, lifecycle variants, pinned edges, revision/link snapshots, history, drafts and receipts, then applies 0008. It compares deterministically ordered full rows for every durable graph table and verifies exact projection eligibility/live-child counts, ready candidates, and the administrator projection check. The entire private schema is rolled back and its removal is checked. See [E2 evidence](implementation/E2.md) for the current result. This database-level upgrade test does not establish previous-image rollback or deployment-stack upgrade timing.
