"""Vulcan -- which model answered, for which seat, at what cost.

Vulcan is the provenance join in the workflow Mosaic exists to answer: given a
claim, which call drafted it and who paid. `GET /v1/usage` carries ledger totals
with per-model provider attribution.

Vulcan self-describes too, and its document does not match Minerva's:
`api_version` + `callable_capabilities`, with no `schema_version` at all. Two
adopters, two schemas. `Capability.raw` keeps both verbatim so a shape we failed
to normalize is inspectable rather than lost.
"""

from __future__ import annotations

from .. import config
from .base import Capability, Entry, Unreachable, unreachable_from
from .transport import ReadOnlyHTTP
from .transport import Unreachable as TransportUnreachable


class VulcanAdapter:
    module = "vulcan"

    def __init__(self) -> None:
        self._endpoint = config.resolve(self.module)
        self._http = ReadOnlyHTTP(module=self.module, base_url=self._endpoint.address)

    def capabilities(self) -> Capability:
        try:
            caps = self._http.get_json("/v1/capabilities")
        except TransportUnreachable:
            return Capability(module=self.module, listener=None)
        if not isinstance(caps, dict):
            caps = {}
        return Capability(
            module=self.module,
            # Deliberately not defaulted to a made-up schema version: Vulcan's
            # document has none, and inventing one would manufacture agreement
            # between the fleet's only two capability documents.
            version=caps.get("schema_version") or caps.get("api_version"),
            surfaces=tuple(caps.get("callable_capabilities") or ()),
            listener=self._endpoint.address,
            raw=caps,
        )

    def read(self) -> tuple[tuple[Entry, ...], tuple[Unreachable, ...]]:
        """No entries, by design -- and reachable, which is a different fact.

        Vulcan holds no work, no decisions, and no claims: nothing that belongs
        on an operator's desk. It is consulted when tracing a specific claim back
        to the call that drafted it. A module appearing on the desk merely
        because it is reachable is noise.
        """
        try:
            self._http.get_json("/v1/capabilities")
        except TransportUnreachable as exc:
            return (), (unreachable_from(exc, self.module),)
        return (), ()
