"""Composition: adapter results in, one `desk.v1` document out.

**Zero I/O.** `compose()` and `to_document()` are pure functions of their
arguments: no network, no filesystem, no clock. That is what makes the
document's rules testable without a live fleet, and it is why the only
impure thing here -- `compose_desk()` -- does nothing but fetch adapter
results and hand them to `compose()`.

The discipline, inherited from Athena's desk: **it composes, it does not
compute.** Nothing in this module derives a fact, reconciles a disagreement
between two modules, or fills in a missing value. If two modules disagree,
both statements appear. If a join is missing, it is reported missing.
Synthesizing a link to make the document look complete is the single most
damaging thing this tool could do.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

from beacon_legacy.contracts.desk import SCHEMA, Desk, Entry, Unreachable

if TYPE_CHECKING:
    from beacon_legacy.contracts.adapter import Adapter

#: One adapter's return: what it read, and what it could not.
AdapterResult = tuple[tuple[Entry, ...], tuple[Unreachable, ...]]


def compose(
    results: Iterable[AdapterResult],
    *,
    generated_at: str,
) -> Desk:
    """Merge adapter results into one document. Pure.

    Entries are ordered by `timestamp` descending -- **recency, explicitly not
    importance**. Beacon legacy desk does not rank authority (docs/authority.md), and an
    ordering that put "stronger" approvals on top would be exactly that
    ranking, smuggled in as a display detail. Ties keep adapter order, so the
    output is deterministic for a fixed input.
    """
    entries: list[Entry] = []
    unreachable: list[Unreachable] = []
    for got, missed in results:
        entries.extend(got)
        unreachable.extend(missed)

    ordered = sorted(entries, key=lambda e: e.timestamp, reverse=True)
    return Desk(
        schema=SCHEMA,
        generated_at=generated_at,
        entries=tuple(ordered),
        unreachable=tuple(unreachable),
    )


def to_document(desk: Desk) -> dict[str, Any]:
    """Serialize to the plain mapping renderers consume. Pure.

    Renderers take the document, never the contract classes -- deliberate, so
    the display layer depends on the *document*, and any shell that later
    renders a desk inherits the authority rules without importing Beacon legacy desk
    internals.

    StrEnum members serialize as their string values, so the mapping is
    JSON-ready with no custom encoder.
    """
    return asdict(desk)


def compose_desk() -> dict[str, Any]:
    """Compose a live desk from the registered adapters.

    The one impure function in this module, and the entry point
    `beacon-legacy-desk desk` calls. It performs no composition logic of its own: it
    gathers adapter results and delegates to `compose()`.

    The adapter registry belongs to Lane B, so it is imported lazily -- this
    module stays importable, and every rule in it stays testable, before any
    adapter exists.

    An absent adapter layer does NOT raise. It is reported as an unreachable
    module in an otherwise valid document, because that is what this project
    says a missing join is: a result. `beacon-legacy-desk doctor` names it too. Crashing
    here would make "nothing is wired up yet" indistinguishable from "the tool
    is broken", which is the same lie in a different place.
    """
    from datetime import UTC, datetime

    generated_at = datetime.now(UTC).isoformat(timespec="seconds")

    try:
        from beacon_legacy.adapters import registry  # Lane B
    except ImportError as exc:
        return to_document(
            compose(
                [
                    (
                        (),
                        (
                            Unreachable(
                                module="beacon_legacy.adapters",
                                reason="the adapter layer has not landed yet",
                                detail=f"owned by lane B; import failed: {exc}",
                            ),
                        ),
                    )
                ],
                generated_at=generated_at,
            )
        )

    adapters: tuple[Adapter, ...] = registry()
    results = [adapter.read() for adapter in adapters]
    return to_document(compose(results, generated_at=generated_at))
