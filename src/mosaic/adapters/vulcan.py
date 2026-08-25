"""Vulcan -- which model answered, for which seat, at what cost.

Vulcan is the provenance join in the workflow Mosaic exists to answer: given a
claim, which call drafted it and who paid. `GET /v1/usage` carries the ledger
totals and a per-model breakdown with provider attribution.

Vulcan also serves `GET /v1/capabilities`, and its shape does not match
Minerva's: `api_version` + `callable_capabilities`, no `schema_version` field.
Two self-describing modules, two schemas. Mosaic normalises rather than
declaring a standard that its only two adopters already disagree on.
"""

from __future__ import annotations

from .. import config
from .base import Entry, ModuleProbe
from .transport import ReadOnlyHTTP, Unreachable


class VulcanAdapter:
    module = "vulcan"

    def __init__(self) -> None:
        self._endpoint = config.resolve(self.module)
        self._http = ReadOnlyHTTP(module=self.module, base_url=self._endpoint.address)

    def probe(self) -> ModuleProbe:
        try:
            caps = self._http.get_json("/v1/capabilities")
        except Unreachable as exc:
            return ModuleProbe(
                module=self.module,
                reachable=False,
                address=exc.address,
                address_source=self._endpoint.source,
                detail=exc.reason,
            )
        if not isinstance(caps, dict):
            caps = {}
        callable_caps = caps.get("callable_capabilities") or []
        return ModuleProbe(
            module=self.module,
            reachable=True,
            address=self._endpoint.address,
            address_source=self._endpoint.source,
            detail="serves " + (", ".join(callable_caps) if callable_caps else "nothing declared"),
            version=caps.get("api_version"),
            # Deliberately None: Vulcan's capability document has no schema
            # version. Reporting one here would invent agreement that does not
            # exist between the fleet's two capability documents.
            schema_version=caps.get("schema_version"),
        )

    def entries(self) -> list[Entry]:
        """Empty by design.

        Vulcan holds no work, no decisions, and no claims -- nothing that belongs
        on an operator's desk. It is consulted when tracing a specific claim back
        to the call that drafted it, which is `mosaic trace`, not `mosaic desk`.
        A module appearing on the desk merely because it is reachable is noise.
        """
        return []
