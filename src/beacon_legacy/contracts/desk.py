"""The `desk.v1` envelope: one read-only cross-module document.

Modelled deliberately on Athena's agent desk (`athena.agent_desk.v1`), which
is a working implementation of the discipline we need rather than a design we
had to invent. Two conventions are taken from it directly:

- **It composes, it does not compute.** Every entry is the owning module's own
  read. Beacon legacy desk can never show something the owning module would not, and can
  never disagree with the page it points at. Nothing here is a second source
  of truth.
- **Every entry states its own `meaning`,** in the module's words.

To which Beacon legacy desk adds one of its own, because it reads across trust boundaries
Athena's desk does not:

- **A missing join is a result, not an error.** `unreachable` is part of the
  document. Papering over an absent link with a synthesized one is the single
  most damaging thing this tool could do.

Field names (`source`, `owner`, `title`, `status`, `timestamp`,
`schema_version`, `ref`, `link`) match what Lane C's renderers already
consume. Conforming to the existing shape beat imposing a nicer one.

The document is a flat, ranked entry list, not Athena's lane structure:
grouping is a rendering concern, and the roadmap asks for "a read-only desk.v1
document assembled through two real adapters", not a second Athena.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from beacon_legacy.contracts.authority import Authority

SCHEMA = "desk.v1"


@dataclass(frozen=True, slots=True)
class Provenance:
    """Where this was read from, precisely enough to re-derive it.

    An entry that cannot name its source is a bug (ADR 0001).
    """

    source: str
    """The exact endpoint or revision read: `"http://host:18300/desk"`, or
    `"git:/path/to/icarus@af41796"`. Never a floating name like `HEAD` --
    binding a name rather than a commit makes drift fire on every unrelated
    change, which teaches the reader to ignore drift (ADR 0001)."""

    read_at: str
    """ISO-8601 UTC. When Beacon legacy desk read it, NOT when the fact was created."""

    module_version: str | None = None
    """What the module reports as its own version or commit, when it offers
    one. Athena's `GET /version` returns `commit` and `tree_state`."""

    tree_state: str | None = None
    """`"clean"`, `"dirty"`, or None when the module does not report it.
    First-class because it is the same distinction ADR 0001 turns on: a clean
    tree is a published artifact, a dirty one is somebody's in-progress
    state."""


@dataclass(frozen=True, slots=True)
class Entry:
    """One fact from one module.

    Carries every field the roadmap requires of a desk entry: source module,
    owner, status, timestamp, schema version, canonical link, authority label.

    Note the division of labour between `status` and `authority`: `status` is
    the module's own word for where this thing stands, and `authority` -- when
    present -- is the approval vocabulary. An entry with no approval is never
    forced to borrow an approval word to describe itself.
    """

    source: str
    owner: str
    title: str

    status: str
    """The module's OWN status word, verbatim: Janus `superseded`, Athena
    `in_progress`, whatever the module calls it. Never mapped onto Beacon legacy desk's
    vocabulary.

    This was typed `Lifecycle` for four commits and that was wrong twice over.

    It forced every module to borrow an *approval* vocabulary to describe
    things that are not approvals. An Athena issue is work; its `done` has no
    honest home in a five-value approval enum, and picking one either
    understates it or asserts a check nobody ran. Worse, Janus's three
    terminal-negative states -- `withdrawn`, `expired`, `superseded` -- all
    collapsed onto `PROPOSED`, so **4 of 13 live gates rendered identically to
    a live open gate**. A question nobody has answered and a question the
    world moved past are different facts, and that is this project's own
    failure mode one level up from the one it was built to prevent.

    It also duplicated `Authority.lifecycle`, which already carries the
    approval vocabulary and is populated only when there *is* an approval.
    Two fields holding the same concept can disagree, and one of them was
    being populated by a lossy mapping table living in adapters -- which is
    doctrine, in the wrong place.

    Found by Lane B against live data, exactly as agreed: bring the case
    rather than invent a label.
    """

    timestamp: str
    """ISO-8601 UTC, from the MODULE -- when the fact happened. Distinct from
    `provenance.read_at`, which is when Beacon legacy desk looked."""

    schema_version: str
    """The OWNING module's schema version for this payload, e.g. `"gate.v1"`,
    `"claim.v2"`. Not Beacon legacy desk's."""

    ref: str
    provenance: Provenance
    link: str | None = None
    authority: Authority | None = None
    """None when the entry carries no approval at all -- the common case, and
    NOT the same as an approval that failed to verify. A renderer must not
    show these alike."""

    payload: dict[str, Any] = field(default_factory=dict)
    """The module's own projection, verbatim. Deliberately unmodelled, for the
    reason Athena gives for not declaring a response_model on its desk:
    re-declaring another surface's shape creates a second place for it to
    drift."""


@dataclass(frozen=True, slots=True)
class Unreachable:
    """A module or join Beacon legacy desk could not read, and why.

    A first-class part of the document. v0 succeeds if `beacon-legacy-desk doctor` names
    precisely which joins do not exist yet -- a failing join is a result, not
    a bug. Never synthesize a link to fill one of these in.
    """

    module: str
    reason: str
    detail: str | None = None


@dataclass(frozen=True, slots=True)
class Desk:
    """The whole read-only document."""

    generated_at: str
    schema: str = SCHEMA
    entries: tuple[Entry, ...] = ()
    unreachable: tuple[Unreachable, ...] = ()

    @property
    def complete(self) -> bool:
        """True only when every module answered. False is a normal outcome,
        and the renderer must say so rather than implying a whole picture."""
        return not self.unreachable
