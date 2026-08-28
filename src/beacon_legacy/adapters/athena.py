"""Athena -- the work an operator and their agents have committed to.

Athena is the module whose own Desk endpoint Beacon legacy desk is modelled on, and the
discipline that endpoint states -- "it composes, it does not compute" -- is the
rule this adapter follows too. Every field emitted here is a field Athena
returned.

**Athena is identity-scoped.** A token minted for an agent seat sees that seat's
view, not the operator's. Beacon legacy desk must never present Athena's contribution as the
whole of Athena; `capabilities()` reports which token spelling was used so the
scope of what was read is visible rather than assumed.

Athena issues carry no approval, so every entry here has `authority=None`. That
is the contract's common case and it is deliberately not the same as an approval
that failed to verify.

`status` is Athena's own word -- `open`, `in_progress`, `done` -- passed through
untouched. It used to be mapped onto the roadmap's five-value approval enum,
which forced a work item to describe itself in approval vocabulary; Lane A ruled
the field back to a plain string after this adapter's mapping table showed why.
"""

from __future__ import annotations

from typing import Any

from .. import config
from .base import (
    Capability,
    Entry,
    Unreachable,
    provenance,
    unreachable_from,
)
from .transport import ReadOnlyHTTP
from .transport import Unreachable as TransportUnreachable


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

    # -- discovery ---------------------------------------------------------

    def capabilities(self) -> Capability:
        try:
            version = self._http.get_json("/version")
        except TransportUnreachable:
            return Capability(module=self.module, listener=None)
        if not isinstance(version, dict):
            version = {}
        return Capability(
            module=self.module,
            version=str(version.get("version")) if version.get("version") else None,
            surfaces=("/version", "/desk", "/issues", "/approvals", "/events"),
            listener=self._endpoint.address,
            raw={
                "address_source": self._endpoint.source,
                # Which env var supplied the token, never the token itself.
                "token_source": self._endpoint.token_source,
                "identity_scoped": True,
                "commit": version.get("commit"),
                "tree_state": version.get("tree_state"),
            },
        )

    # -- reading -----------------------------------------------------------

    def read(self) -> tuple[tuple[Entry, ...], tuple[Unreachable, ...]]:
        base = self._endpoint.address.rstrip("/")
        issues_path = f"/issues?status=open&limit={self._limit}"
        source = f"{base}{issues_path}"

        # Version first: it is the provenance every entry carries, and a module
        # serving from a dirty tree is serving something no commit describes.
        module_version: str | None = None
        tree_state: str | None = None
        try:
            version = self._http.get_json("/version")
            if isinstance(version, dict):
                module_version = version.get("commit") or version.get("version")
                tree_state = version.get("tree_state")
        except TransportUnreachable as exc:
            return (), (unreachable_from(exc, self.module),)

        try:
            payload = self._http.get_json(issues_path)
        except TransportUnreachable as exc:
            return (), (unreachable_from(exc, self.module),)

        issues: list[Any]
        if isinstance(payload, list):
            issues = payload
        elif isinstance(payload, dict) and isinstance(payload.get("items"), list):
            issues = payload["items"]
        else:
            return (), (
                Unreachable(
                    module=self.module,
                    reason="/issues answered with neither a list nor an {items: [...]} object",
                    detail=source,
                ),
            )

        entries = tuple(
            self._entry(issue, base, source, module_version, tree_state) for issue in issues
        )
        return entries, ()

    def _entry(
        self,
        issue: dict[str, Any],
        base: str,
        source: str,
        module_version: str | None,
        tree_state: str | None,
    ) -> Entry:
        status = str(issue.get("status", ""))
        return Entry(
            source=self.module,
            owner=str(issue.get("assignee_name") or "unassigned"),
            title=str(issue.get("title", "")),
            status=status,
            timestamp=str(issue.get("created_at") or ""),
            schema_version="athena.issue.v1",
            ref=str(issue.get("key") or issue.get("id", "")),
            provenance=provenance(source, module_version=module_version, tree_state=tree_state),
            link=f"{base}/aegis/issues/{issue.get('id')}",
            # An Athena issue is work, not an approval. Athena's approvals are a
            # separate mechanism on /approvals and are not read here.
            authority=None,
            payload={
                "project": issue.get("project_name"),
                "priority": issue.get("priority"),
                "assignee_is_agent": issue.get("assignee_is_agent"),
                "labels": issue.get("labels") or [],
            },
        )
