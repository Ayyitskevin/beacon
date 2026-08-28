"""Shared machinery for adapters, and the one place Lane B talks to the contract.

Every adapter builds its entries through the helpers here, so the contract has a
single import site in this lane rather than six.

**On `Entry.status`.** It is a plain string: the owning module's own word, passed
through untouched. No adapter maps it, and no adapter invents one. The approval
vocabulary lives on `Authority.lifecycle` and is populated only when there is an
approval, so an entry that carries no approval borrows no approval word to
describe itself.
"""

from __future__ import annotations

from datetime import UTC, datetime

from beacon_legacy.contracts import (
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


def now_iso() -> str:
    """When Beacon legacy desk looked. Distinct from when the fact happened."""

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
