#!/bin/sh
set -eu
binary=$1
shift
case "$binary" in
  "$E2_TARGET_ROOT"/*) relative=${binary#"$E2_TARGET_ROOT"/} ;;
  *) echo 'unexpected executable outside the isolated target' >&2; exit 1 ;;
esac
exec docker run --rm --network container:aq-graph-e2-db \
  --cpus=2 --memory=1g --pids-limit=256 --read-only --tmpfs /tmp \
  -v "$E2_TARGET_ROOT:/target:ro" \
  -e AQ_GRAPH_TEST_DATABASE_URL=postgres://aq_graph@127.0.0.1:5432/aq_graph_test \
  -e RUST_TEST_THREADS=2 \
  ubuntu@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3 \
  "/target/$relative" "$@"
