"""Minerva -- research memory, and one of two modules that already self-describe.

Minerva serves `GET /api/v1/capabilities` returning `minerva.capabilities.v3`:
listener mode, identity boundary, egress posture, and the argv for each surface.
It is the closest thing the fleet has to a discovery convention.

It is also where Mosaic learns that a convention with two members is not yet a
convention -- see capabilities.py. Vulcan self-describes too, with a different
shape and no `schema_version` at all.

Known live defect, measured 2026-08-24 and not worked around here: the fleet
entrypoint ~/.local/bin/minerva injects `--db` into every verb outside a small
allowlist, and `capabilities` takes no `--db`, so the CLI discovery path Minerva
itself advertises (`capabilities_argv: ["capabilities"]`) fails with usage_error.
The venv binary is fine. Mosaic reads capabilities over HTTP, so it is unaffected
-- but it must not silently paper over a broken discovery path in a module whose
discovery document it is relying on. `mosaic doctor` reports it.
"""

from __future__ import annotations

from .. import config
from .base import Entry, ModuleProbe
from .transport import ReadOnlyHTTP, Unreachable


class MinervaAdapter:
    module = "minerva"

    def __init__(self) -> None:
        self._endpoint = config.resolve(self.module)
        self._http = ReadOnlyHTTP(module=self.module, base_url=self._endpoint.address)

    def probe(self) -> ModuleProbe:
        try:
            caps = self._http.get_json("/api/v1/capabilities")
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
        listener = caps.get("listener") or {}
        mode = listener.get("mode", "unknown") if isinstance(listener, dict) else "unknown"
        return ModuleProbe(
            module=self.module,
            reachable=True,
            address=self._endpoint.address,
            address_source=self._endpoint.source,
            detail=f"listener mode {mode}",
            version=caps.get("api_version"),
            schema_version=caps.get("schema_version"),
        )

    def entries(self) -> list[Entry]:
        """Empty by design, for now.

        Minerva's claims are the evidence half of the killer workflow, but they
        hang off a mission and are reached as /missions/{id}/claims. Emitting a
        flat claim list without its mission would produce entries that look
        joinable and are not. An empty lane with a stated reason is a result;
        a plausible lane that cannot be traced back is a lie.
        """
        return []
