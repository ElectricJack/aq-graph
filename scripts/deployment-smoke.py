#!/usr/bin/env python3
"""Disposable, isolated Compose deployment/recovery drill (Docker + Python stdlib)."""

import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
PROJECT = os.environ.get("AQ_GRAPH_SMOKE_PROJECT", "aq-graph-validation")
PORT = int(os.environ.get("AQ_GRAPH_SMOKE_PORT", "18746"))
IMAGE = os.environ.get("AQ_GRAPH_SMOKE_IMAGE", "aq-graph:validation")

ENV = dict(
    {k: v for k, v in os.environ.items() if not k.startswith("COMPOSE_")},
    COMPOSE_PROJECT_NAME=PROJECT,
    AQ_GRAPH_PORT=str(PORT),
    AQ_GRAPH_IMAGE=IMAGE,
)


def validate_settings():
    if not __debug__:
        raise SystemExit(
            "Run without Python optimization; validation assertions are required"
        )
    dedicated_port = (1 <= PORT <= 65535) and PORT != 8741
    dedicated_project = re.fullmatch(r"aq-graph-validation[a-z0-9_-]*", PROJECT)
    if not dedicated_port or not dedicated_project:
        raise SystemExit("Use a dedicated validation project and nondefault port")


def check_unused_project():
    # Never attach to or clean up a stack that existed before this invocation.
    label = f"label=com.docker.compose.project={PROJECT}"
    for command in (["ps", "-aq"], ["volume", "ls", "-q"], ["network", "ls", "-q"]):
        matches = subprocess.check_output(
            ["docker", *command, "--filter", label], text=True
        )
        if matches.strip():
            raise SystemExit("Validation project already exists; refusing to touch it")


def recovery_request(a, b, child):
    return {
        "idempotency_key": "drill-create",
        "operations": [
            {
                "op": "create_node",
                "id": a,
                "kind": "task",
                "title": "Recovery task",
                "task_state": "active",
            },
            {
                "op": "create_node",
                "id": b,
                "kind": "knowledge",
                "title": "Recovery evidence",
            },
            {
                "op": "create_node",
                "id": child,
                "kind": "task",
                "title": "Recovery child",
                "task_state": "active",
            },
            {
                "op": "add_edge",
                "id": str(uuid.uuid4()),
                "source": a,
                "target": b,
                "edge_type": "references",
                "target_revision": 1,
            },
            {
                "op": "add_edge",
                "id": str(uuid.uuid4()),
                "source": a,
                "target": child,
                "edge_type": "parent-child",
            },
        ],
    }


def await_restored(check, transient_errors, timeout_message):
    for _ in range(60):
        try:
            check()
            return
        except transient_errors:
            time.sleep(1)
    raise AssertionError(timeout_message)


class Stack:
    def __init__(self, work):
        self.work = work

    def run(self, *args, check=True, capture=False):
        return subprocess.run(
            args,
            cwd=self.work,
            env=ENV,
            check=check,
            text=True,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.PIPE if capture else None,
        )

    def compose(self, *args, **kwargs):
        return self.run(
            "docker",
            "compose",
            "--project-directory",
            str(self.work),
            "--file",
            str(self.work / "compose.yaml"),
            "--project-name",
            PROJECT,
            *args,
            **kwargs,
        )

    def prepare(self):
        for name in ("compose.yaml",):
            shutil.copy(ROOT / name, self.work / name)
        (self.work / "scripts").mkdir()
        for name in ("local-init.sh", "backup.sh", "restore.sh"):
            shutil.copy(ROOT / "scripts" / name, self.work / "scripts" / name)
        self.run("bash", "scripts/local-init.sh")
        assert (self.work / ".local").stat().st_mode & 0o777 == 0o700
        assert (self.work / ".local/token").stat().st_mode & 0o777 == 0o600
        assert (
            json.loads((self.work / ".local/config.json").read_text())["endpoint"]
            == f"http://127.0.0.1:{PORT}"
        )
        secrets = {p.name: p.read_bytes() for p in (self.work / ".local").iterdir()}
        self.run("bash", "scripts/local-init.sh")
        assert secrets == {
            p.name: p.read_bytes() for p in (self.work / ".local").iterdir()
        }
        if not os.environ.get("AQ_GRAPH_SMOKE_SKIP_BUILD"):
            subprocess.run(["docker", "build", "-t", IMAGE, str(ROOT)], check=True)
        self.compose("config", "--quiet")

    def inspect(self):
        graph = self.compose("ps", "-q", "graph", capture=True).stdout.strip()
        inspection = json.loads(
            subprocess.check_output(["docker", "inspect", graph], text=True)
        )[0]
        assert inspection["Config"]["Image"] == IMAGE
        assert inspection["HostConfig"]["ReadonlyRootfs"]
        assert inspection["Config"]["User"].split(":")[0] != "0"
        assert (
            self.compose(
                "exec",
                "-T",
                "graph",
                "sh",
                "-c",
                "touch /must-fail",
                check=False,
                capture=True,
            ).returncode
            != 0
        )
        pg = self.compose("ps", "-q", "postgres", capture=True).stdout.strip()
        pg_info = json.loads(
            subprocess.check_output(["docker", "inspect", pg], text=True)
        )[0]
        assert not pg_info["HostConfig"]["PortBindings"]
        return inspection["Image"]

    def recovery_client(self):
        token = (self.work / ".local/token").read_text()
        base = f"http://127.0.0.1:{PORT}/v1/workspaces/00000000-0000-4000-8000-000000000001"

        def api(path, data=None):
            req = urllib.request.Request(
                base + path,
                data=json.dumps(data).encode() if data is not None else None,
                headers={
                    "Authorization": "Bearer " + token,
                    "Content-Type": "application/json",
                },
            )
            with urllib.request.urlopen(req, timeout=15) as response:
                return json.load(response)

        def mutate(key, operations):
            return api("/mutations", {"idempotency_key": key, "operations": operations})

        return api, mutate

    def seed(self):
        api, mutate = self.recovery_client()
        a, b, child = (str(uuid.uuid4()) for _ in range(3))
        request = recovery_request(a, b, child)
        receipt = api("/mutations", request)
        n = api("/nodes/" + a)
        mutate(
            "drill-summary",
            [
                {
                    "op": "set_summary",
                    "id": a,
                    "expected_version": n["version"],
                    "source_revision": n["content_revision"],
                    "summary": "Retained summary",
                }
            ],
        )
        draft = api(
            "/changesets",
            {
                "idempotency_key": "drill-draft",
                "name": "Retained proposal",
                "operations": [
                    {
                        "op": "update_node",
                        "id": b,
                        "expected_version": 1,
                        "patch": {"body": "Proposed edit"},
                    }
                ],
            },
        )
        draft_id = draft.get("id", draft.get("change_set_id"))
        assert draft_id, draft
        paths = [
            "/nodes/" + a,
            "/nodes/" + b,
            "/nodes/" + a + "/history?limit=100",
            "/nodes/" + a + "/revisions/2",
            "/nodes/" + a + "/revisions/3",
            "/nodes/" + a + "/revisions/4",
            "/nodes/" + a + "/diff?from=1&to=3",
            "/nodes/" + child,
            "/ready?limit=20",
            "/changesets/" + draft_id,
        ]
        expected = {p: api(p) for p in paths}
        assert [n["id"] for n in expected["/ready?limit=20"]["items"]] == [child]

        def verify():
            assert {p: api(p) for p in paths} == expected
            assert api("/mutations", request) == receipt

        return verify, mutate

    def verify_restart(self, verify):
        self.compose("restart", "graph")
        self.compose("up", "-d", "--no-build", "--wait", "--wait-timeout", "120")
        verify()
        self.compose(
            "up",
            "-d",
            "--no-build",
            "--force-recreate",
            "graph",
            "--wait",
            "--wait-timeout",
            "120",
        )
        verify()

    def database_snapshot(self, database):
        tables = self.compose(
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            "aq_graph",
            "-d",
            database,
            "-Atc",
            "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename",
            capture=True,
        ).stdout.splitlines()
        return {
            table: self.compose(
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                "aq_graph",
                "-d",
                database,
                "-Atc",
                f"SELECT count(*), md5(COALESCE(string_agg(row_to_json(t)::text, chr(10) ORDER BY row_to_json(t)::text), '')) FROM \"{table}\" t",
                capture=True,
            ).stdout.strip()
            for table in tables
        }

    def assert_script_refused(self, script, *args):
        result = self.run("bash", script, *args, check=False, capture=True)
        assert result.returncode != 0

    def restore_backup(self):
        source_snapshot = self.database_snapshot("aq_graph")
        self.run("bash", "scripts/backup.sh", ".local/drill.dump")
        self.assert_script_refused("scripts/backup.sh", ".local/drill.dump")
        (self.work / ".local/checksum-only.dump.sha256").write_text("must remain")
        self.assert_script_refused("scripts/backup.sh", ".local/checksum-only.dump")
        assert (
            self.work / ".local/checksum-only.dump.sha256"
        ).read_text() == "must remain"
        # Corrupt checksum and valid-checksum invalid archive both fail safely.
        invalid = self.work / ".local/invalid.dump"
        invalid.write_bytes(b"not a PostgreSQL archive")
        Path(str(invalid) + ".sha256").write_text("0" * 64 + "\n")
        self.assert_script_refused(
            "scripts/restore.sh", str(invalid), "aq_graph_restore_bad_checksum"
        )
        Path(str(invalid) + ".sha256").write_text(
            hashlib.sha256(invalid.read_bytes()).hexdigest() + "\n"
        )
        self.assert_script_refused(
            "scripts/restore.sh", str(invalid), "aq_graph_restore_bad_archive"
        )
        self.run(
            "bash", "scripts/restore.sh", ".local/drill.dump", "aq_graph_restore_drill"
        )
        self.assert_script_refused(
            "scripts/restore.sh", ".local/drill.dump", "aq_graph_restore_drill"
        )
        self.assert_script_refused(
            "scripts/restore.sh", ".local/drill.dump", "aq_graph"
        )
        assert self.database_snapshot("aq_graph_restore_drill") == source_snapshot
        return source_snapshot

    def verify_clone(self, verify, mutate):
        # Run the same image against the clone, without bootstrap, then verify exact API values.
        self.compose("stop", "graph")
        self.compose(
            "run",
            "-d",
            "--no-deps",
            "--service-ports",
            "--name",
            PROJECT + "-restored",
            "--entrypoint",
            "/bin/sh",
            "graph",
            "-ec",
            'unset AQ_GRAPH_BOOTSTRAP_WORKSPACE AQ_GRAPH_BOOTSTRAP_ACTOR AQ_GRAPH_BOOTSTRAP_TOKEN_FILE; export DATABASE_URL="postgres://aq_graph:$(cat /run/secrets/postgres_password)@postgres:5432/aq_graph_restore_drill"; exec aq-graph serve',
        )
        await_restored(
            verify, (OSError, AssertionError), "Restored service did not verify"
        )

        def write():
            return mutate(
                "drill-restored-write",
                [
                    {
                        "op": "create_node",
                        "kind": "task",
                        "title": "Write on restored clone",
                    }
                ],
            )

        restored_write = write()
        self.run("docker", "restart", PROJECT + "-restored")

        def replay():
            assert write() == restored_write
            verify()

        await_restored(replay, OSError, "Restored restart did not verify")

    def report(self, source_snapshot, image_id, started):
        print(
            json.dumps(
                {
                    "result": "passed",
                    "image": IMAGE,
                    "project": PROJECT,
                    "port": PORT,
                    "checks": [
                        "credential rerun",
                        "nonroot/read-only",
                        "private PostgreSQL",
                        "fresh start",
                        "restart",
                        "same-schema recreate",
                        "exact revisions/links/history/draft/summary",
                        "ready leaf projection",
                        "receipt replay",
                        "backup/restore",
                        "invalid restore",
                        "existing destination refusal",
                        "restored write",
                    ],
                    "database_snapshot": source_snapshot,
                    "image_id": image_id,
                    "postgres_version": self.compose(
                        "exec",
                        "-T",
                        "postgres",
                        "psql",
                        "-U",
                        "aq_graph",
                        "-d",
                        "aq_graph",
                        "-Atc",
                        "SHOW server_version",
                        capture=True,
                    ).stdout.strip(),
                    "elapsed_seconds": round(time.monotonic() - started, 2),
                },
                indent=2,
            )
        )


def drill(work):
    started = time.monotonic()
    stack = Stack(work)
    check_unused_project()
    stack.prepare()
    try:
        stack.compose("up", "-d", "--no-build", "--wait", "--wait-timeout", "180")
        image_id = stack.inspect()
        verify, mutate = stack.seed()
        stack.verify_restart(verify)
        snapshot = stack.restore_backup()
        stack.verify_clone(verify, mutate)
        stack.report(snapshot, image_id, started)
    finally:
        # Compose labels restrict cleanup to resources created by this invocation.
        stack.compose("down", "--volumes", "--remove-orphans")


def main():
    validate_settings()
    with open("/tmp/" + PROJECT + ".lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with tempfile.TemporaryDirectory(prefix="aq-graph-deployment-") as work:
            drill(Path(work))


if __name__ == "__main__":
    main()
