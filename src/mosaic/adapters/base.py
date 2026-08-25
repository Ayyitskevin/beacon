"""Shared machinery for adapters, and the one place Lane B talks to the contract.

Every adapter builds its entries through the helpers here, so the contract has a
single import site in this lane rather than six.

**On `Entry.status`.** The contract types it as `Lifecycle`, whose five values are
mandated by the roadmap: proposed / reviewed / authorized / executed / verified.
Two of the four live modules have statuses that do not fit it -- see
`LIFECYCLE_IS_LOSSY` below. Where an adapter must choose, it chooses the least
misleading value AND preserves the module's own word verbatim in
`payload["module_status"]`. No adapter invents a lifecycle value, and no adapter
silently discards the module's own vocabulary; a test asserts the raw word survives.
"""

from __future__ import annotations

from datetime import UTC, datetime

from mosaic.contracts import (
    Attachment,
    Authority,
    Capability,
    Decider,
    Enforcement,
    Entry,
    Lifecycle,
    Provenance,
    Unreachable,
)

#: Documented so it is a known cost rather than a silent one. Raised to Lane A
#: with the live counts; until the contract changes, the mapping tables in each
#: adapter are the whole of the lossiness and `payload["module_status"]` is the
#: escape hatch.
LIFECYCLE_IS_LOSSY = (
    "Lifecycle has no terminal-negative value. Janus's refused / withdrawn / "
    "expired / superseded and Athena's open / in_progress / done have to be "
    "mapped onto an approval vocabulary they were not drawn from. 4 of 13 live "
    "gates are superseded -- a third of the corpus lands on a forced fit."
)


def now_iso() -> str:
    """When Mosaic looked. Distinct from when the fact happened."""

    return datetime.now(UTC).isoformat(timespec="seconds")


def provenance(
    source: str,
    *,
    module_version: str | None = None,
    tree_state: str | None = None,
) -> Provenance:
    """Name the exact endpoint or revision this came from (ADR 0001).

    `source` is the endpoint, command, or pinned revision -- never just the
    module name. An entry that cannot say where it came from is a bug.
    """

    return Provenance(
        source=source,
        read_at=now_iso(),
        module_version=module_version,
        tree_state=tree_state,
    )


def unreachable_from(exc: Exception, module: str) -> Unreachable:
    """Turn a transport failure into the document's own failure type.

    Adapters never raise out of `read()`. An empty read and a failed read are
    different facts about the world, and a desk that renders them alike is lying
    by omission.
    """

    reason = getattr(exc, "reason", None) or str(exc)
    address = getattr(exc, "address", None)
    return Unreachable(module=module, reason=str(reason), detail=address)


__all__ = [
    "LIFECYCLE_IS_LOSSY",
    "Attachment",
    "Authority",
    "Capability",
    "Decider",
    "Enforcement",
    "Entry",
    "Lifecycle",
    "Provenance",
    "Unreachable",
    "now_iso",
    "provenance",
    "unreachable_from",
]
