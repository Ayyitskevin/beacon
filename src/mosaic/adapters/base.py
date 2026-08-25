"""What every adapter is, and the one seam where Lane A's contracts plug in.

An adapter answers two questions about one module and nothing else:

    probe()   -- are you there, at the address I tried, and what are you running?
    entries() -- what do you currently hold that belongs on the operator's desk?

Deliberately absent: any method that changes anything. The adapter protocol has
no write verb, so "Mosaic cannot write to your module" is enforced by the shape
of the interface rather than by everyone remembering.

CONTRACT SEAM: `Entry` below is provisional and owned by Lane A
(src/mosaic/contracts/). When that lands, this file's `Entry` and `entry()` are
replaced by an import, and no adapter changes -- every adapter builds entries
through `entry()` and never constructs one directly. The churn is one function.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Protocol, runtime_checkable

# Provisional. Lane A's docs/authority.md decides the real label set; adapters
# must not invent labels or hardcode these strings anywhere but here.
UNLABELLED = "unlabelled"


@dataclass(frozen=True)
class ModuleProbe:
    """Whether a module answered, and what it says it is.

    `address` is always populated, including on failure -- see transport.Unreachable.
    """

    module: str
    reachable: bool
    address: str
    address_source: str
    detail: str = ""
    version: str | None = None
    commit: str | None = None
    tree_state: str | None = None
    schema_version: str | None = None


@dataclass(frozen=True)
class Entry:
    """One thing a module holds, described in the module's own terms.

    Every field the roadmap requires of a desk entry is mandatory here except
    `authority_label`, which defaults to UNLABELLED precisely so that an adapter
    that has not thought about authority produces a visibly unlabelled entry
    rather than a plausible-looking wrong one.
    """

    source: str  # owning module
    kind: str  # what the module calls this thing
    ref: str  # the module's own identifier
    title: str
    status: str  # the module's own status string, never normalised away
    owner: str | None
    timestamp: str | None
    schema_version: str | None
    canonical_link: str
    authority_label: str = UNLABELLED
    binding: dict[str, str] = field(default_factory=dict)
    extra: dict[str, object] = field(default_factory=dict)


def entry(**kwargs: object) -> Entry:
    """The single construction site for entries.

    Adapters call this, never `Entry(...)`. When Lane A's contract lands, this
    body changes and six adapters do not.
    """

    return Entry(**kwargs)  # type: ignore[arg-type]


@runtime_checkable
class Adapter(Protocol):
    module: str

    def probe(self) -> ModuleProbe: ...

    def entries(self) -> list[Entry]: ...
