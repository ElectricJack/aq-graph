# aq-graph

A standalone task and knowledge graph service and CLI, designed for many agents sharing one authoritative graph.

Start with the [service specification](docs/specs/0001-aq-graph.md). The proposed implementation uses Rust and PostgreSQL, with selective history and measured performance under concurrent load.

This repository currently contains the specification only. It will own the graph domain, service, CLI, database migrations, API contract, standalone deployment, and performance tests. Agent Q will consume its versioned API and own its orchestration-specific adapter and deployment configuration.

The repository is independent of Agent Q. Local development and operation must not require an Agent Q installation.
