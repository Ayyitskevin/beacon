"""The record envelope: identity, provenance, references, relationships.

One store, one spine. Every capability's records sit in this envelope.

The governing rule, which reads at four levels and is the reason most of this
file looks the way it does:

    An absence with no place to be recorded makes distinct absences
    indistinguishable.

  1. field      -- no nullable carries meaning, and no defaulted boolean either
  2. reference  -- a missing join is a queryable finding, not a dangling pointer
  3. record     -- did the thing that did not happen get written down?
  4. capability -- did the thing that was never built get represented at all?

Levels 3 and 4 cannot be enforced by a type: a capability that only records
successes has made failure structurally invisible, and a registry listing only
what was built cannot say what was not. They are here as named obligations for
the layers that can.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
from typing import Any


class Capability(StrEnum):
    """The seven. Closed, and it includes ones that do not exist yet.

    ReplayDeck is in this enum despite never having been built. That is level
    4 of the absence rule: it was ranked first in the portfolio, named in the
    product trace, and every handoff said "six capabilities" for a full day --
    because a capability that does not exist has nowhere to be absent from.
    A registry derived from what happens to be implemented cannot report what
    is missing.
    """

    CONVERSATION = "conversation"
    WORK = "work"
    EVIDENCE = "evidence"
    PROVENANCE = "provenance"
    EVALUATION = "evaluation"
    AUTHORITY = "authority"
    EXECUTION = "execution"


class Origin(StrEnum):
    """Which system produced a record. Closed; adding one is deliberate.

    Third part of a reference for a reason that bites later rather than now:
    the same capability holds records from more than one origin, indefinitely.
    `MWS-2` migrated from the old Athena and `MWS-2` created natively in
    Mosaic afterwards are different records with the same ref, and two parts
    cannot separate them.
    """

    NATIVE = "native"
    MIGRATED_BUZZ = "migrated:buzz"
    MIGRATED_ATHENA = "migrated:athena"
    MIGRATED_MINERVA = "migrated:minerva"
    MIGRATED_VULCAN = "migrated:vulcan"
    MIGRATED_REPLAYDECK = "migrated:replaydeck"
    MIGRATED_JANUS = "migrated:janus"
    MIGRATED_ICARUS = "migrated:icarus"
    EXTERNAL = "external"


class Resolution(StrEnum):
    """Whether a reference points at something we hold. Four-valued."""

    RESOLVED = "resolved"
    ABSENT = "absent"
    """We know what it names and do not hold it -- never imported, or never
    will be. **The common, healthy state**, not an error. This is what keeps
    "2 of 7 trace hops joinable" measurable: a foreign key that refused an
    un-imported reference would delete the measurement, and a nullable that
    swallowed it would hide it."""

    BROKEN = "broken"
    """It named something that should exist and does not."""

    UNRESOLVED = "unresolved"
    """Not resolved this run. Transient."""


@dataclass(frozen=True, slots=True)
class Reference:
    """A pointer at a record, as a triple. Never a bare string.

    Athena keys (`MWS-2`), Buzz event ids (64 hex) and Janus gate ids
    (`g3410c2cff0f`) are all "an id" and none is unique across the set.

    **Resolution is not destructive.** The triple is retained and
    `resolved_to` is recorded alongside it -- never in place of it. Replacing
    the triple with a store id on resolution feels like normalization and
    destroys two things at once: the ability to re-check, and any way to
    detect that the resolution was wrong. A wrong resolution that has eaten
    its own input is undetectable forever.

    That is the same rule as "a re-evaluated verdict is never persisted as a
    stored fact": the triple is the fact, `resolved_to` is a conclusion drawn
    from it at a moment.
    """

    capability: Capability
    ref: str
    origin: Origin
    resolution: Resolution
    resolved_to: str
    """The store id when `resolution is RESOLVED`. Otherwise the empty string
    -- an explicit non-value rather than a null, because a null here would be
    doing the work of four different resolution states."""

    def __post_init__(self) -> None:
        if self.resolution is Resolution.RESOLVED and not self.resolved_to:
            raise ValueError("a RESOLVED reference must name what it resolved to")
        if self.resolution is not Resolution.RESOLVED and self.resolved_to:
            raise ValueError(
                f"reference is {self.resolution} but carries resolved_to="
                f"{self.resolved_to!r} -- a stale conclusion outliving its verdict"
            )


class RelationKind(StrEnum):
    """Why two records are related. Three meanings that were one field.

    Structural belonging, access scope and reply-threading were all converging
    on a single parent reference. Any two of them conflated is correct-looking
    until someone's access follows a structural move.
    """

    CONTAINS = "contains"
    """Structural belonging: where a record lives. At most one per record."""

    SCOPES = "scopes"
    """What governs who may see or act. **Never implicitly inherited from
    CONTAINS** -- a capability that wants scope-follows-containment declares
    it. A structural move must never silently change who can see a record."""

    REPLIES_TO = "replies_to"
    """Conversational order. May cross containment boundaries; threading is
    not containment and forcing them to agree mis-models conversation."""

    DERIVES_FROM = "derives_from"
    REFERENCES = "references"


@dataclass(frozen=True, slots=True)
class Relation:
    """One typed edge. Separate from the record, so a record can have many."""

    from_record: str
    to: Reference
    kind: RelationKind


@dataclass(frozen=True, slots=True)
class Provenance:
    """How this fact came to exist, precisely enough to re-derive it.

    Under federation this answered "which endpoint or revision did we read".
    That question is gone; this one is harder and matters more. A record that
    cannot name where it came from is a bug.
    """

    origin: Origin
    source_ref: str
    """The natural key in the originating system. Empty string when there is
    none -- an explicit non-value, and never the store's own id."""

    source_rev: str
    """The exact revision the fact was taken from, where one exists."""

    actor: str
    """Who or what produced it."""

    imported_at: str
    """Set when `origin` is a migration, empty otherwise. Imported and native
    records coexist indefinitely -- capabilities retire on their own schedules
    -- so this is permanent, not transitional, and must never be inferable
    only from a timestamp."""


@dataclass(frozen=True, slots=True)
class Record:
    """One fact, in the one envelope.

    `id` is store-assigned and opaque. Natural keys live in
    `provenance.source_ref`: a migration that imports the same gate twice, or
    two capabilities whose ref formats overlap, must not collide -- and a
    record's identity must not change if its source system is corrected.
    """

    id: str
    capability: Capability
    kind: str
    """The capability's own record type, its word. Free text scoped by
    capability, never a shared taxonomy -- a shared enum every capability must
    squeeze into is the most expensive mistake available at this layer."""

    title: str
    status: str
    """A member of this `(capability, kind)`'s closed vocabulary, or
    UNMAPPED_LEGACY. Empty string for `event`-natured types, which declare no
    status at all rather than carrying a one-member vocabulary."""

    occurred_at: str
    """When the fact happened, per its source."""

    recorded_at: str
    """When this store learned it. Two clocks, never one: a migration gives
    every imported record the same `recorded_at` and wildly different
    `occurred_at`, and collapsing them makes imported and native records
    indistinguishable while ordering silently means different things."""

    provenance: Provenance
    body: dict[str, Any]
    """The typed structure for this `(capability, kind)`. Typed because an
    untyped column is the nullable problem at larger scale -- every key is a
    nullable field and none of them can be constrained. Declared as a mapping
    here; the per-type schemas live with the capability sub-models."""

    body_unmapped: dict[str, Any]
    """Content we did not model, carried verbatim with its reason. An explicit
    state, never a silent omission and never a coercion to the nearest thing
    we did model. A migration that quietly drops an unmodelled field is
    permanent data loss with no second look coming."""
