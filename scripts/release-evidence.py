#!/usr/bin/env python3
"""Bind a pushed OCI image to its source and immutable build inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tomllib


def write_evidence(root: Path, output: Path, image: str, digest: str,
                   tag: str, revision: str) -> dict:
    with (root / 'Cargo.toml').open('rb') as source:
        version = tomllib.load(source)['package']['version']
    if tag != 'v' + version:
        raise ValueError('release tag must match Cargo package version')
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', digest):
        raise ValueError('pushed image digest must be sha256 plus 64 lowercase hex digits')
    if not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise ValueError('source revision must be a full Git SHA')
    if not re.fullmatch(r'ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+', image):
        raise ValueError('image must be an untagged GHCR repository')
    bases = []
    stages = set()
    for line in (root / 'Dockerfile').read_text().splitlines():
        parts = line.split()
        if not parts or parts[0].upper() != 'FROM':
            continue
        base = parts[1]
        if base not in stages:
            if not re.fullmatch(r'[^\s@]+@sha256:[0-9a-f]{64}', base):
                raise ValueError('every external Docker base must be digest-pinned')
            bases.append(base)
        if len(parts) == 4 and parts[2].upper() == 'AS':
            stages.add(parts[3])
    if len(bases) != 2 or not bases[0].startswith('rust:1.96.1-bookworm@') or not bases[1].startswith('debian:bookworm-slim@'):
        raise ValueError('expected Rust 1.96.1 bookworm and Debian bookworm-slim bases')
    record = {
        'release': tag,
        'image_digest': digest,
        'image_reference': f'{image}@{digest}',
        'source_revision': revision,
        'rust_toolchain': '1.96.1',
        'target': 'x86_64-unknown-linux-gnu',
        'platform': 'linux/amd64',
        'cargo_lock_sha256': hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest(),
        'dependency_inventory': 'Cargo.lock',
        'base_images': bases,
        'api_version': 1,
        'schema_version': 8,
    }
    output.mkdir(parents=True, exist_ok=True)
    (output / 'Cargo.lock').write_bytes((root / 'Cargo.lock').read_bytes())
    (output / 'release-provenance.json').write_text(json.dumps(record, indent=2) + '\n')
    (output / 'image-reference.txt').write_text(record['image_reference'] + '\n')
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path('.'))
    parser.add_argument('--output', type=Path, required=True)
    for name in ['image', 'digest', 'tag', 'revision']:
        parser.add_argument('--' + name, required=True)
    write_evidence(**vars(parser.parse_args()))


if __name__ == '__main__':
    main()
