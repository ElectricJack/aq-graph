"""Release identity must be complete and reject mutable or malformed inputs."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_evidence', Path(__file__).with_name('release-evidence.py'))
release_evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_evidence)


class ReleaseEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'Cargo.toml').write_text('[package]\nversion = "0.1.0"\n')
        (self.root / 'Cargo.lock').write_text('committed locked dependencies\n')
        self.bases = ['rust:1.96.1-bookworm@sha256:' + 'a' * 64,
                      'debian:bookworm-slim@sha256:' + 'b' * 64]
        (self.root / 'Dockerfile').write_text(f'FROM {self.bases[0]} AS dependencies\nFROM dependencies AS build\nFROM {self.bases[1]}\n')
        self.args = dict(root=self.root, output=self.root / 'artifacts',
                         image='ghcr.io/electricjack/aq-graph', digest='sha256:' + 'c' * 64,
                         tag='v0.1.0', revision='d' * 40)

    def test_evidence_binds_digest_to_source_toolchain_lockfile_and_bases(self):
        record = release_evidence.write_evidence(**self.args)
        saved = json.loads((self.args['output'] / 'release-provenance.json').read_text())
        self.assertEqual(saved, record)
        self.assertEqual(record['release'], 'v0.1.0')
        self.assertEqual(record['image_digest'], self.args['digest'])
        self.assertEqual(record['image_reference'], self.args['image'] + '@' + self.args['digest'])
        self.assertEqual(record['source_revision'], 'd' * 40)
        self.assertEqual(record['rust_toolchain'], '1.96.1')
        self.assertEqual(record['target'], 'x86_64-unknown-linux-gnu')
        self.assertEqual(record['cargo_lock_sha256'], hashlib.sha256((self.root / 'Cargo.lock').read_bytes()).hexdigest())
        self.assertEqual(record['base_images'], self.bases)
        self.assertEqual(record['dependency_inventory'], 'Cargo.lock')
        self.assertEqual((self.args['output'] / 'Cargo.lock').read_bytes(), (self.root / 'Cargo.lock').read_bytes())
        self.assertEqual((record['api_version'], record['schema_version']), (1, 8))
        self.assertEqual((self.args['output'] / 'image-reference.txt').read_text(), record['image_reference'] + '\n')
        self.assertFalse(any('token' in key.lower() for key in record))

    def test_rejects_bad_release_identity_before_creating_output(self):
        for key, value in [('tag', 'v0.2.0'), ('digest', 'latest'),
                           ('digest', 'sha256:' + 'g' * 64), ('revision', 'main'),
                           ('image', 'ghcr.io/electricjack/aq-graph:latest')]:
            with self.subTest(key=key, value=value):
                with self.assertRaises(ValueError):
                    release_evidence.write_evidence(**(self.args | {key: value}))
                self.assertFalse(self.args['output'].exists())

    def test_rejects_unpinned_base_before_creating_output(self):
        (self.root / 'Dockerfile').write_text('FROM rust:1.96.1-bookworm AS build\nFROM debian:bookworm-slim\n')
        with self.assertRaises(ValueError):
            release_evidence.write_evidence(**self.args)
        self.assertFalse(self.args['output'].exists())


if __name__ == '__main__':
    unittest.main()
