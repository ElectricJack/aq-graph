"""Small dependency-free Python adapter for AQ or another orchestrator.

One instance per worker/thread; HTTP connections are reused and are not thread safe.
This adapter never claims work or reads aq-graph database tables.
"""
import http.client
import json
import ssl
import urllib.parse
import uuid


class GraphError(Exception):
    def __init__(self, status, code, message):
        self.status, self.code = status, code
        super().__init__(message)


class GraphClient:
    def __init__(self, endpoint, workspace, token, timeout=12):
        url = urllib.parse.urlsplit(endpoint)
        if url.scheme not in ("http", "https") or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError("endpoint must be HTTP(S), without embedded credentials")
        if url.scheme == "http" and url.hostname not in ("localhost", "127.0.0.1", "::1"):
            raise ValueError("use HTTPS for remote clients")
        self._connection = (
            http.client.HTTPSConnection(url.hostname, url.port, timeout=timeout, context=ssl.create_default_context())
            if url.scheme == "https" else http.client.HTTPConnection(url.hostname, url.port, timeout=timeout)
        )
        self._prefix = url.path.rstrip("/") + "/v1/workspaces/" + str(uuid.UUID(str(workspace)))
        self._token = token

    def close(self):
        self._connection.close()

    def _request(self, method, path, payload=None):
        body = json.dumps(payload).encode() if payload is not None else None
        try:
            self._connection.request(method, self._prefix + path, body, {
                "Authorization": "Bearer " + self._token,
                "Content-Type": "application/json",
                "X-AQ-Graph-API-Version": "1",
            })
            response = self._connection.getresponse()
            data = response.read(2_097_153)
        except (OSError, http.client.HTTPException):
            self.close()
            # The caller retains its idempotency key and exact payload for retries.
            raise GraphError(0, "transport", "Graph request failed; mutation outcome may be unknown") from None
        if len(data) > 2_097_152 or response.getheader("x-aq-graph-api-version") != "1":
            self.close()
            raise GraphError(response.status, "protocol", "Incompatible or oversized graph response")
        try:
            value = json.loads(data)
        except ValueError:
            raise GraphError(response.status, "protocol", "Graph returned invalid JSON") from None
        if response.status >= 400:
            error = value.get("error", {})
            raise GraphError(response.status, error.get("code", "unknown"), error.get("message", "Graph request failed"))
        return value

    def ready(self, limit=50, cursor=None):
        """Candidates only. AQ must apply policy and acquire its own execution claim."""
        query = {"limit": limit}
        if cursor:
            query["cursor"] = cursor
        return self._request("GET", "/ready?" + urllib.parse.urlencode(query))

    def context(self, task_id, depth=1, limit=50):
        task_id = str(uuid.UUID(str(task_id)))
        return self._request("GET", f"/nodes/{task_id}/context?" + urllib.parse.urlencode({"depth": depth, "limit": limit}))

    def mutate(self, operations, *, idempotency_key, reason=None):
        """Persist the key and exact operations in AQ before calling this method."""
        return self._request("POST", "/mutations", {
            "idempotency_key": idempotency_key, "reason": reason, "operations": operations,
        })

    def complete(self, task_id, expected_version, *, idempotency_key, reason=None):
        return self.mutate([{
            "op": "update_node", "id": str(uuid.UUID(str(task_id))),
            "expected_version": expected_version, "patch": {"task_state": "completed"},
        }], idempotency_key=idempotency_key, reason=reason)
