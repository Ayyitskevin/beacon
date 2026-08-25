"""Minerva -- research memory, and one of two modules that already self-describe.

Minerva serves `GET /api/v1/capabilities` returning `minerva.capabilities.v3`:
listener mode, identity boundary, egress posture, and the argv for each surface.
It is the closest thing the fleet has to a discovery convention -- and, with
Vulcan's differently-shaped document, the reason ADR 0002 says normalize rather
than declare one.

Known live defect, not worked around here: the fleet entrypoint
`~/.local/bin/minerva` injects `--db` into every verb outside a small allowlist,
and `capabilities` takes none -- so the CLI discovery path Minerva itself
advertises fails, while the venv binary works. Mosaic reads over HTTP and is
unaffected, but `mosaic doctor` re-measures and reports it, because a broken
discovery path in a module whose discovery document we rely on is our problem too.
"""

from __future__ import annotations

from .. import config
from .base import Capability, Entry, Unreachable, unreachable_from
from .transport import ReadOnlyHTTP
from .transport import Unreachable as TransportUnreachable


class MinervaAdapter:
    module = "minerva"

    def __init__(self) -> None:
        self._endpoint = config.resolve(self.module)
        self._http = ReadOnlyHTTP(module=self.module, base_url=self._endpoint.address)

    def capabilities(self) -> Capability:
        try:
            caps = self._http.get_json("/api/v1/capabilities")
        except TransportUnreachable:
            return Capability(module=self.module, listener=None)
        if not isinstance(caps, dict):
            caps = {}
        listener = caps.get("listener")
        mode = listener.get("mode") if isinstance(listener, dict) else None
        surfaces = tuple((caps.get("surfaces") or {}).keys()) if isinstance(caps, dict) else ()
        return Capability(
            module=self.module,
            version=caps.get("schema_version") or caps.get("api_version"),
            surfaces=surfaces,
            listener=f"{self._endpoint.address} ({mode})" if mode else self._endpoint.address,
            raw=caps,
        )

    def read(self) -> tuple[tuple[Entry, ...], tuple[Unreachable, ...]]:
        """Reachable, and deliberately contributing nothing to the desk yet.

        Minerva's claims are the evidence half of the workflow Mosaic exists to
        answer, but they hang off a mission and are reached as
        `/missions/{id}/claims`. Emitting a flat claim list without its mission
        would produce entries that look joinable and are not.

        This returns no entries and no unreachable: Minerva answered, and it has
        nothing for this desk. That is a third state, and the contract has room
        for it precisely so it does not have to be faked as either of the others.
        """
        try:
            self._http.get_json("/api/v1/capabilities")
        except TransportUnreachable as exc:
            return (), (unreachable_from(exc, self.module),)
        return (), ()
