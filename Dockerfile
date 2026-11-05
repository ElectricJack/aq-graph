FROM rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS dependencies
WORKDIR /build
# Cargo profile: release images use `release` (ThinLTO). CI-only images pass
# --build-arg CARGO_PROFILE=ci-image (release without LTO; see Cargo.toml).
ARG CARGO_PROFILE=release
COPY Cargo.toml Cargo.lock ./
# Cache locked dependencies separately from application source and migrations.
# Clean only this package so the later build cannot reuse the placeholder binary.
RUN mkdir src \
    && printf 'fn main() {}\n' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --locked --profile "$CARGO_PROFILE" --bin aq-graph \
    && cargo clean --profile "$CARGO_PROFILE" --package aq-graph

FROM dependencies AS build
ARG CARGO_PROFILE=release
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --profile "$CARGO_PROFILE" --bin aq-graph \
    && cp "target/$CARGO_PROFILE/aq-graph" /build/aq-graph

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
ARG AQ_GRAPH_VERSION=0.1.1
LABEL org.opencontainers.image.title="aq-graph" \
      org.opencontainers.image.description="Standalone task and knowledge graph service and CLI" \
      org.opencontainers.image.version="${AQ_GRAPH_VERSION}" \
      io.aq-graph.api-version="1" \
      io.aq-graph.schema-version="8"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 aqgraph && useradd --uid 10001 --gid aqgraph --no-create-home aqgraph
COPY --from=build /build/aq-graph /usr/local/bin/aq-graph
USER 10001:10001
ENV AQ_GRAPH_BIND=0.0.0.0:8741
EXPOSE 8741
HEALTHCHECK --interval=10s --timeout=6s --start-period=30s --retries=5 CMD ["aq-graph", "--endpoint", "http://127.0.0.1:8741", "health"]
ENTRYPOINT ["aq-graph"]
CMD ["serve"]
