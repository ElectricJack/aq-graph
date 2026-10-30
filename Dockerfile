FROM rust:1.96.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --release --bin aq-graph

FROM debian:bookworm-slim
ARG AQ_GRAPH_VERSION=0.1.0
LABEL org.opencontainers.image.title="aq-graph" \
      org.opencontainers.image.description="Standalone task and knowledge graph service and CLI" \
      org.opencontainers.image.version="${AQ_GRAPH_VERSION}" \
      io.aq-graph.api-version="1" \
      io.aq-graph.schema-version="3"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 aqgraph && useradd --uid 10001 --gid aqgraph --no-create-home aqgraph
COPY --from=build /build/target/release/aq-graph /usr/local/bin/aq-graph
USER 10001:10001
ENV AQ_GRAPH_BIND=0.0.0.0:8741
EXPOSE 8741
HEALTHCHECK --interval=10s --timeout=6s --start-period=30s --retries=5 CMD ["aq-graph", "--endpoint", "http://127.0.0.1:8741", "health"]
ENTRYPOINT ["aq-graph"]
CMD ["serve"]
