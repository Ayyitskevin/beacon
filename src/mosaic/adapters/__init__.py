"""The adapter registry -- including the modules that do not exist yet.

AI Town is **seven** capabilities, not six. The roadmap's product trace is
Buzz -> Athena -> Minerva -> Vulcan -> ReplayDeck -> Janus -> Icarus, and
ReplayDeck (the `fleet-eval` repo) is in it at the evaluation step. Almost every
handoff written about this work says six and quietly drops it.

So the registry lists all seven, and each one declares how it is read and whether
an adapter exists. A module with no adapter is a **row in the report**, not an
absence from it -- the roadmap is explicit that knowing precisely which joins are
missing is worth more than a shell, and a registry that only lists what we built
cannot tell anyone what we did not.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

from mosaic.contracts import Adapter, Capability, Entry, Unreachable

from .athena import AthenaAdapter
from .janus import JanusAdapter
from .minerva import MinervaAdapter
from .transport import READ_VERBS, ReadOnlyCommand, ReadOnlyHTTP
from .vulcan import VulcanAdapter


@dataclass(frozen=True)
class Registration:
    module: str
    owns: str  # the noun this module owns, from the roadmap
    access: str  # how Mosaic reads it, or why it cannot
    factory: Callable[[], Adapter] | None  # None == no adapter yet
    absent_reason: str = ""


REGISTRY: tuple[Registration, ...] = (
    Registration(
        module="buzz",
        owns="live conversation, identity, mentions",
        access="relay HTTP + the `buzz` CLI",
        factory=None,
        absent_reason=(
            "No adapter in v0. Buzz's conversation reads go through the `buzz` CLI "
            "under an owner key; reading a channel is a credentialed act, and the "
            "two real adapters the first slice calls for are cheaper to get right."
        ),
    ),
    Registration(
        module="athena",
        owns="missions, work, operator documents",
        access="HTTP, Bearer token",
        factory=AthenaAdapter,
    ),
    Registration(
        module="minerva",
        owns="evidence, claims, uncertainty",
        access="HTTP, loopback, unauthenticated",
        factory=MinervaAdapter,
    ),
    Registration(
        module="vulcan",
        owns="inference aliases, admission, usage",
        access="HTTP, loopback, unauthenticated",
        factory=VulcanAdapter,
    ),
    Registration(
        module="replaydeck",
        owns="deterministic evaluation and replay",
        access="none -- the repo does not exist",
        factory=None,
        absent_reason=(
            "Ranked #1 in the portfolio and named in the product trace, but never "
            "built. `gh repo list` shows no fleet-eval or ReplayDeck repo. This row "
            "exists so the gap is reported rather than forgotten."
        ),
    ),
    Registration(
        module="janus",
        owns="pending human decisions",
        access="the `janus` CLI, read verbs only",
        factory=JanusAdapter,
    ),
    Registration(
        module="icarus",
        owns="bounded execution and receipts",
        access="committed git objects at an explicit revision",
        factory=None,
        absent_reason=(
            "No daemon and nothing listening -- Icarus is repo-only, and no run "
            "state exists on this host. ADR 0001 permits reading its committed git "
            "objects at a pinned revision, never its working tree. Deferred to the "
            "slice after v0, when there is a receipt worth reading."
        ),
    ),
)


def available() -> list[Registration]:
    return [r for r in REGISTRY if r.factory is not None]


def registry() -> tuple[Adapter, ...]:
    """Every adapter that exists, instantiated.

    This is the name `mosaic.compose.compose_desk` imports lazily, so landing
    this function is what lights up `mosaic desk`. It returns only the modules
    with adapters; the ones without are not silently dropped -- `mosaic doctor`
    reports them from REGISTRY, with a reason each.
    """

    return tuple(r.factory() for r in REGISTRY if r.factory is not None)


__all__ = [
    "READ_VERBS",
    "REGISTRY",
    "Adapter",
    "AthenaAdapter",
    "Capability",
    "Entry",
    "JanusAdapter",
    "MinervaAdapter",
    "ReadOnlyCommand",
    "ReadOnlyHTTP",
    "Registration",
    "Unreachable",
    "VulcanAdapter",
    "available",
    "registry",
]
