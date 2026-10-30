# Deployment and recovery validation

The repeatable command is:

```sh
python3 scripts/deployment-smoke.py
```

This runs the same script as CI. Locally it builds the production Dockerfile, uses the Compose PostgreSQL 17 image, generates isolated credentials in a temporary directory, and creates only project `aq-graph-validation` resources on loopback port 18746. Existing containers, volumes or networks under that project cause refusal before mutation. A local project lock prevents concurrent script invocations. Compose routing environment variables are removed and all direct Compose calls use explicit manifest, directory and project arguments. Helper scripts inherit the isolated project and run in the temporary copy. Cleanup on normal completion or Python exception removes only the owned disposable project. Forced process termination can leave those resources for explicit inspection and cleanup; the next run refuses to adopt them. Optimized Python mode is refused so validation assertions cannot silently disappear.

CI builds `aq-graph:validation` with Buildx, restores/exports the GitHub Actions cache, and loads that image into the runner's Docker daemon. It then runs the drill with `AQ_GRAPH_SMOKE_SKIP_BUILD=1` and the same explicit image tag. This keeps all deployment/recovery assertions while avoiding another image build inside the drill. The deployment job runs independently of the Rust/PostgreSQL checks; workflow success and reusable release checks require both jobs to pass.

The Dockerfile builds locked dependencies from `Cargo.toml`/`Cargo.lock` before copying application source or migrations. It deletes this package's placeholder artifacts before the real build, retaining dependency artifacts in the cached layer. Source or migration edits therefore invalidate the application layer; manifest, lockfile or toolchain changes also invalidate dependencies. Release publishing can read the same image-cache scope. Cache hits accelerate compilation but never skip the recovery drill.

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
