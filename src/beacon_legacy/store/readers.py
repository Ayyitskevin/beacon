"""The migration-reader interface: six systems, read once, faithfully.

**The premise changed and these rules were re-derived, not inherited.** The
adapters this replaces were written for *live modules Beacon legacy desk did not own*, under
`docs/adr/0001-read-boundary.md`. Kevin's ruling voids that ADR — there are no
module boundaries in a unified codebase — and the adapters survive only as the
code that knows how to get data out of systems that are retiring. Every rule
below is stated with the reason it holds *now*, because a rule carried forward
with its original reason is how a correct rule becomes a wrong one.

**Read-only, for a new reason.** It was a safety property: do not corrupt a
live module. It is now also an *idempotency* property: the migration must be
re-runnable across a multi-month tail, and a reader that writes to its source
makes the second run differ from the first. The rule survives with stronger
justification than it had.

**Surface choice is per-source and must be justified, not assumed.** With the
ADR void, reading a source's database directly is no longer forbidden. That
does not make it preferred. The argument for it was that an API returns derived
verdicts while the store needs their inputs — but measured against Athena, the
desk API emits `expires_at` (the input) beside `claim_state` (the verdict), so
that argument does not hold there. The rule is therefore narrower and
checkable: **for every derived value, confirm the input is reachable through
the surface this reader uses.** Where it is not, that is the argument for a
different surface, per source, on evidence.

**A reader is the last layer that holds the source's own vocabulary.** After
its system retires, nothing else can distinguish what the source said from what
Beacon legacy desk decided it meant.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum
from typing import Protocol, runtime_checkable


class SurfaceKind(StrEnum):
    """How a reader reaches its source. A CLOSED vocabulary, not a free string.

    It was a bare `str`, and claude-09 caught what that costs: `require_quiescent`
    keys on it, so a reader declaring `kind="file"` while opening SQLite — or
    simply typing `"localdb"` — passed straight through the guard and never
    triggered the quiescence check. A typo silently skipped a safety check, in
    a store whose entire doctrine is closed vocabularies. The guard consulted a
    self-report and believed it.

    Closing it is only half. The other half is structural: 0d keys their AST
    contract on the `sqlite3.connect` capability rather than on this declared
    kind, so a mislabeled reader cannot dodge it either. Runtime and structure
    have to agree, because each alone is evadable by a different mistake.
    """

    FILE = "file"
    LOCAL_DB = "local_db"
    HTTP = "http"
    SUBPROCESS = "subprocess"


@dataclass(frozen=True)
class SourceSurface:
    """How one reader reaches its source, and why that surface was chosen."""

    kind: SurfaceKind
    """Closed. `__post_init__` refuses anything outside the vocabulary rather
    than coercing it: coercion is how a typo becomes a plausible value."""

    boundary: str
    """How read-only is enforced *for this surface*. Not one uniform rule:
    a process boundary is real against a file or a local database, and buys
    nothing against a credentialed HTTP endpoint, where a process holding a
    bearer to read can also POST with it. There the boundary is the credential
    — a read-only-scoped token bound to one endpoint."""

    quiescence: Quiescence | None = None
    """Required for `local_db`, meaningless otherwise. `None` means nobody
    checked, which `require_quiescent` treats as unsafe rather than as
    probably-fine."""

    justification: str = ""
    """Why this surface rather than another, in evidence. Specifically: which
    derived values this source has, and whether their inputs are reachable
    here. A surface chosen by convention is a decision nobody made."""

    def __post_init__(self) -> None:
        # Refuse rather than coerce. A coerced typo becomes a plausible value,
        # and the guard downstream cannot tell it from a deliberate one.
        if not isinstance(self.kind, SurfaceKind):
            raise ValueError(
                f"surface kind {self.kind!r} is not one of "
                f"{[k.value for k in SurfaceKind]}. It is a closed vocabulary "
                f"because require_quiescent keys on it: an unrecognised kind "
                f"would skip the quiescence check silently."
            )


@dataclass(frozen=True)
class Quiescence:
    """Evidence that a `local_db` source is safe to read directly.

    ADR 0001 forbade opening another module's database because doing so
    bypasses that module's scopes, authorization and activity trail, and
    observes mid-transaction state. Every one of those is a property of a
    **live** system. A stopped service or a frozen copy has none of them, so
    the ADR is a **precondition, not a prohibition** — which is the same
    re-derivation that kept read-only while changing its reason.

    Fail closed: a reader confirms quiescence and refuses otherwise. It never
    assumes, because the failure mode of assuming is a torn read that looks
    like valid data.
    """

    method: str
    """`frozen_snapshot` (preferred) or `service_stopped`."""

    evidence: str
    """How it was CHECKED, re-runnably. Not an assertion that it is true — a
    command whose output said so."""

    verified: bool
    """False until something actually checked. A default of True would make
    the precondition decorative."""


class SourceNotQuiescent(Exception):
    """A `local_db` read was attempted against a source not shown to be still.

    Raised rather than warned. A torn read from a live SQLite file does not
    announce itself: it produces rows that parse, validate, and are wrong.
    """


def require_quiescent(surface: SourceSurface) -> None:
    """Refuse a direct database read unless quiescence was actually verified.

    Raises:
        SourceNotQuiescent: if the surface is `local_db` and carries no
            verified quiescence. Non-database surfaces pass through untouched.
    """
    if surface.kind is not SurfaceKind.LOCAL_DB:
        return
    q = surface.quiescence
    if q is None or not q.verified:
        raise SourceNotQuiescent(
            f"refusing to open {surface.kind} directly: quiescence was not "
            f"verified. Stop the service or read a frozen snapshot, and record "
            f"how that was checked. Reading a live SQLite file can tear across "
            f"a write and produce rows that parse and are wrong."
        )


@dataclass(frozen=True)
class SourceRecord:
    """One record as the source system actually presents it.

    Deliberately not the store's shape. Mapping happens after reading, so that
    the unmapped original is always available to the mapper — and so a reader
    can be tested against a captured fixture without the store existing.
    """

    origin: str
    origin_ref: str
    origin_status: str
    """The source's OWN status word, verbatim. Carried unconditionally, not
    only when it fails to map. The Janus collapse happened on a *successful*
    mapping: `superseded` mapped cleanly onto `REVIEWED`, no refusal fired, and
    the information died inside a valid mapping. A successful lossy mapping is
    the one nobody inspects."""

    payload: dict = field(default_factory=dict)
    derived_inputs: dict = field(default_factory=dict)
    """The inputs of any value the source computed at read time. A derived
    value whose inputs were not migrated can only be trusted, never
    recomputed."""


@runtime_checkable
class MigrationReader(Protocol):
    """What every reader is. Note what is absent: any method that writes.

    Read-only is a property of the interface shape, not of each reader
    remembering. The protocol surface is checked by a test, so adding a write
    verb here fails the build rather than a review.
    """

    origin: str
    surface: SourceSurface

    def read(self) -> list[SourceRecord]: ...


class UnknownSourceValue(Exception):
    """A source value with no member in the owning lane's vocabulary.

    Raised, never defaulted. This is totality check 3 at the migration ingress:
    the same refuse-don't-default behaviour the database enforces on write and
    the type layer enforces on native reads, at the third ingress point.

    The reader tests against the owning lane's ONE canonical definition, read
    through `vocabulary.canonical_values` — never against a local copy, which
    would be the second source of truth this whole mechanism exists to prevent.
    """


def absent_is_not_loss(member_added_by_beacon_legacy: bool, source_had_value: bool) -> bool:
    """Whether a missing vocabulary member represents actual data loss.

    Measured by claude-0d: only Janus represents terminal-negative as a status
    *value*. Athena carries it in a side column (`archived_at`, set on 0 of 60
    live issues); Minerva as a verb on a sibling record. So members like
    `work/issue: abandoned` and `evidence/claim: withdrawn` exist in Beacon legacy desk's
    vocabulary with **no source value to map from** — marked `added_by_beacon_legacy`
    in the owning lane's definition.

    A reader that cannot tell "we added this member" from "the source didn't
    provide it" reports the added members as data loss on every run. Read the
    marker; never infer it from absence. Absence and 'not applicable here' are
    the same four-state distinction one more time.
    """
    return source_had_value is False and member_added_by_beacon_legacy is False
