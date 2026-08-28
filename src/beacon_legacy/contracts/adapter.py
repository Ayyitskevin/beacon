"""The adapter protocol, and capability discovery.

**Read-only by construction.** This Protocol has no write method, no dispatch
method, and no generic `call`. An adapter that wanted to mutate a module could
not do it through this interface without visibly widening it -- which is the
point. Read-only has to be a property of the shape, not a promise in a README.

That is necessary but not sufficient: nothing here stops an adapter importing
a write-capable client directly. The test that forbids that lives in
`tests/adapters/` and belongs to Lane B, by agreement -- the lane whose code
it constrains owns it.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Protocol, runtime_checkable

from beacon_legacy.contracts.desk import Entry, Unreachable


@dataclass(frozen=True, slots=True)
class Capability:
    """What a module says it can answer, in its own words.

    Two live capability documents already exist and they do NOT agree:
    Minerva's `capabilities.v3` (`schema_version`) and Vulcan's
    `GET /v1/capabilities` (`api_version` + `callable_capabilities`). This
    type normalizes them for Beacon legacy desk's own use; it does not declare a fleet
    standard that two modules already contradict. See ADR 0002.
    """

    module: str
    version: str | None = None
    """Whatever the module calls its own version. Shape varies by module; this
    is recorded, never parsed for meaning."""

    surfaces: tuple[str, ...] = ()
    """Named read surfaces the module advertises."""

    listener: str | None = None
    """Where it is actually listening, or None when nothing is serving.
    Minerva's convention, and the honest answer for a repo-only module like
    Icarus. There is NO uniform local default -- Athena and Buzz answer on the
    tailnet address and refuse loopback; Minerva and Vulcan are loopback-only.
    A bind address is a per-module fact to be discovered or configured, never
    assumed."""

    raw: dict = field(default_factory=dict)
    """The capability document verbatim, so a shape we failed to normalize is
    still inspectable rather than lost."""


@runtime_checkable
class Adapter(Protocol):
    """One module's read surface, as Beacon legacy desk sees it.

    Implementations live in `src/beacon_legacy/adapters/` (Lane B). Everything here
    is a read; there is deliberately no counterpart that writes.
    """

    module: str

    def capabilities(self) -> Capability:
        """What this module advertises. Must not raise for an unreachable
        module -- return a Capability with `listener=None` and let `read`
        report the unreachability. Discovery failing is information."""
        ...

    def read(self) -> tuple[tuple[Entry, ...], tuple[Unreachable, ...]]:
        """Read this module's current facts.

        Returns `(entries, unreachable)`. An adapter that cannot reach its
        module returns `((), (Unreachable(...),))` -- it does NOT raise, and
        it does NOT return an empty tuple alone. An empty read and a failed
        read are different facts about the world, and a desk that renders them
        alike is lying by omission.

        Every returned Entry carries a Provenance naming the exact endpoint or
        revision it came from (ADR 0001).
        """
        ...
