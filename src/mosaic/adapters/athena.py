"""Athena -- the work an operator and their agents have committed to.

Athena is the module whose own Desk endpoint Mosaic is modelled on, and the
discipline that endpoint states -- "it composes, it does not compute" -- is the
rule this adapter follows too. Every field emitted here is a field Athena
returned. Nothing is derived, normalised, or improved.

One honesty note that belongs in the output rather than a docstring, and is
surfaced through probe().detail: Athena authenticates per identity. A token
minted for an agent seat sees that seat's desk, not the operator's. So Athena's
contribution to a cross-module desk is scoped to whoever the token belongs to,
and Mosaic must not present it as the whole of Athena.
"""

from __future__ import annotations

from typing import Any

from .. import config
from .base import Entry, ModuleProbe, entry
from .transport import ReadOnlyHTTP, Unreachable


class AthenaAdapter:
    module = "athena"

    def __init__(self, limit: int = 25) -> None:
        self._endpoint = config.resolve(self.module)
        self._limit = limit
        self._http = ReadOnlyHTTP(
            module=self.module,
            base_url=self._endpoint.address,
            token=self._endpoint.token,
        )

    def probe(self) -> ModuleProbe:
        try:
            version = self._http.get_json("/version")
        except Unreachable as exc:
            return ModuleProbe(
                module=self.module,
                reachable=False,
                address=exc.address,
                address_source=self._endpoint.source,
                detail=exc.reason,
            )
        if not isinstance(version, dict):
            version = {}

        detail = "reachable"
        if not self._endpoint.token:
            detail = "reachable, but no token configured -- reads will be anonymous"
        else:
            detail = f"reachable, token from {self._endpoint.token_source}"

        return ModuleProbe(
            module=self.module,
            reachable=True,
            address=self._endpoint.address,
            address_source=self._endpoint.source,
            detail=detail,
            version=version.get("version"),
            commit=version.get("commit"),
            # Athena reports whether its own checkout is dirty. A module serving
            # from a modified tree is serving something no commit describes, and
            # that belongs on the desk rather than in a log nobody reads.
            tree_state=version.get("tree_state"),
        )

    def entries(self) -> list[Entry]:
        payload = self._http.get_json(f"/issues?status=open&limit={self._limit}")
        issues: list[Any]
        if isinstance(payload, list):
            issues = payload
        elif isinstance(payload, dict) and isinstance(payload.get("items"), list):
            issues = payload["items"]
        else:
            raise Unreachable(
                self.module,
                f"{self._endpoint.address}/issues",
                "answered with neither a list nor an {items: [...]} object",
            )

        base = self._endpoint.address.rstrip("/")
        out: list[Entry] = []
        for issue in issues:
            key = issue.get("key") or str(issue.get("id", ""))
            out.append(
                entry(
                    source=self.module,
                    kind="issue",
                    ref=key,
                    title=issue.get("title", ""),
                    # Athena's own status string, passed through. Mapping it to a
                    # cross-module vocabulary is Lane A's composition step, not an
                    # adapter's -- an adapter that normalises has already decided.
                    status=issue.get("status", ""),
                    owner=issue.get("assignee_name"),
                    timestamp=issue.get("created_at"),
                    schema_version=None,
                    canonical_link=f"{base}/aegis/issues/{issue.get('id')}",
                    extra={
                        "project": issue.get("project_name"),
                        "priority": issue.get("priority"),
                        "assignee_is_agent": issue.get("assignee_is_agent"),
                        "labels": issue.get("labels") or [],
                    },
                )
            )
        return out
