"""The single authority mechanism, as concrete types.

The seven AI Town approval mechanisms collapse into one *type* whose axes can
express all their shapes -- not one shape that replaces them. They were not
seven attempts at the same thing: Icarus binds an execution environment
because that is what it authorizes; Janus binds one artifact and deliberately
does not enforce, to stay out of the permission path; Athena binds an intent
rather than a payload because replaying a stored mutation would be a new trust
boundary; Buzz binds a time-limited token because it gates a step in a run.

Kevin ruled the permission model on 2026-08-25: **report by default, enforce
per-capability for irreversible operations.** `enforcement` therefore stops
being an observation about what some other system does and becomes this
product's own behaviour -- see `beacon_legacy.store.seam`.

Every axis is required. There are no defaults, and that is deliberate: a
defaulted boolean is a two-state nullable with better manners, and the
`spent: bool = False` this replaces is why five superseded gates reported as
live for an hour.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum

from beacon_legacy.store.envelope import Reference


class Decider(StrEnum):
    """Who decided. Not how important the mechanism is."""

    HUMAN = "human"
    POLICY = "policy"
    """A rule evaluated with no person in the loop at decision time."""

    AGENT = "agent"
    """NEVER renders as an approval, whatever it is attached to -- enforced in
    `Authority.assures()` rather than promised in prose. An agent attesting to
    content it computed itself is the highest-consequence case here."""


class Check(StrEnum):
    """What re-deriving the binding produced *this run*.

    Split from the binding itself because they have different lifetimes: a
    binding is fixed at decision time and never changes; a check result
    changes every time you look. Conflating them hides that.
    """

    VERIFIED = "verified"
    DRIFTED = "drifted"
    UNVERIFIABLE = "unverifiable"
    """Can never be re-derived by anyone -- a permanent property of the
    binding. Distinct from UNCHECKED, which is a fact about this run."""

    UNCHECKED = "unchecked"
    """Not re-derived this run. Transient; may resolve either way later."""

    NOT_APPLICABLE = "not_applicable"
    """The binding has nothing to check (intent, handle, step, none)."""


class Enforcement(StrEnum):
    """Who acts when the binding fails. **The operator-action axis.**"""

    ENFORCED = "enforced"
    """This product refuses to proceed. Per Kevin's ruling, reserved for
    operations declared irreversible. Operator action on drift: none required
    to stay safe -- the system already stopped."""

    REPORTED = "reported"
    """States the drift and stops there. Operator action on drift: REQUIRED --
    nothing prevents anyone acting on a void approval."""

    NONE = "none"
    """The mechanism binds no content, so nothing can drift. Its safety comes
    from expiry, single-use consumption or an approver check instead."""


class Lifecycle(StrEnum):
    """Where a decision got to. Distinct from whether it still applies.

    The roadmap's mandated five had no terminal-negative member, which is what
    forced `spent` into existence as a workaround. Kevin ruled the terminal
    member in, with `force` kept distinct (gate gf430f95f410).
    """

    PROPOSED = "proposed"
    REVIEWED = "reviewed"
    AUTHORIZED = "authorized"
    EXECUTED = "executed"
    VERIFIED = "verified"
    CLOSED_NEGATIVE = "closed_negative"
    """Refused, withdrawn, or expired -- reached an end, not favourably."""


class Force(StrEnum):
    """Whether a decision still applies. **Not** where it got to.

    Lifecycle and force answer different questions and neither substitutes for
    the other: a *refused* decision reached a terminal lifecycle and is not in
    force; an *authorized-then-superseded* one reached a favourable lifecycle
    and is also not in force. Collapsing them reproduces "a decision record
    exists" being read as "a decision was favourable".

    Required, with no default. Its predecessor was `spent: bool = False`,
    which held three facts in two states and defaulted to the wrong one.
    """

    PENDING = "pending"
    """No decision reached; "in force" does not apply."""

    IN_FORCE = "in_force"
    SPENT = "spent"
    """Decided, and no longer in force -- superseded, withdrawn, expired,
    refused."""

    INDETERMINATE = "indeterminate"
    """The record is inconsistent; we cannot tell."""


# ---------------------------------------------------------------- bindings --
# A tagged union rather than one struct with nullable fields. An INTENT
# binding has no digest, and "digest is absent" must not be representable as
# a null that could equally mean "digest not recorded" or "digest lost in
# transport". Each variant carries exactly what its kind requires.


@dataclass(frozen=True, slots=True)
class DigestBinding:
    """Binds content by digest. Icarus's shape, and Janus's file/git shape."""

    digest: str
    algorithm: str
    """REQUIRED beside every digest. Icarus's `stableJson` canonicaliser is
    JS-specific: an integral float serializes `5` in Node and `5.0` in Python,
    and non-BMP key ordering differs. Re-deriving a migrated digest with a
    different canonicaliser does not look like a tooling change -- **it looks
    like drift**, which is the one state that demands a human act now. Recording
    the algorithm keeps the vendored-frozen canonicaliser option open."""

    covers: tuple[str, ...]
    """What the digest is over, named. Icarus digests plan + provider + checks
    + sandbox + ceiling + base commit + manifest; Janus digests one artifact.
    Both are "a digest" and they promise different things."""


@dataclass(frozen=True, slots=True)
class IntentBinding:
    """Binds an action against a target, deliberately not its content.

    Athena's shape, and a considered one: replaying a stored mutation would
    re-execute a stale payload under authorization evaluated at a different
    moment. Not a weaker digest -- a different promise.
    """

    action: str
    target: Reference


@dataclass(frozen=True, slots=True)
class HandleBinding:
    """Binds a token. Buzz's shape: time-limited and approver-restricted."""

    handle: str
    expires_at: str
    approver_spec: str


@dataclass(frozen=True, slots=True)
class StepBinding:
    """Binds a position in a run. Buzz's workflow-approval shape."""

    run: Reference
    step_id: str
    step_index: int


@dataclass(frozen=True, slots=True)
class NoBinding:
    """Attached to nothing. An explicit state, never an absence.

    The weakest thing in the system and the easiest to miss, because it
    arrives wearing the same lifecycle as the strongest. Measured 2026-08-25:
    4 of 15 live Janus gates are bound to nothing, and 3 of those are
    `approved`, sitting in the same list as 9 git-bound rulings.
    """

    reason: str
    """Why nothing was bound. Absence of a binding is a fact worth explaining,
    and an unexplained one is indistinguishable from an oversight."""


Binding = DigestBinding | IntentBinding | HandleBinding | StepBinding | NoBinding


#: Checks that must never render as a positive assurance.
NOT_ASSURING: frozenset[Check] = frozenset({Check.DRIFTED, Check.UNVERIFIABLE, Check.UNCHECKED})


class OperatorAction(StrEnum):
    """What a human has to do. Three-valued on purpose.

    A live breach is contained by acting; a standing limitation is closed by
    deciding. A boolean forces one of them to be silent -- and did: an earlier
    boolean answered "no action" for the unbound-and-approved gates this whole
    model was built around.
    """

    ACT = "act"
    DECIDE = "decide"
    NONE = "none"


@dataclass(frozen=True, slots=True)
class Authority:
    """One decision. Every axis required; no defaults anywhere.

    There is deliberately no `is_approved` and no ordering. Reducing these
    axes to one boolean is the collapse this type exists to prevent, and that
    reduction is a doctrine decision rather than an API convenience.
    """

    decider: Decider
    binding: Binding
    check: Check
    enforcement: Enforcement
    lifecycle: Lifecycle
    force: Force
    mechanism: str
    """The mechanism's own name -- `janus.ruling`, `icarus.plan_approval`,
    `buzz.kind.46030`. Buzz kind numbers are recorded literally because Buzz
    is upstream OSS and can renumber; a change must surface as a change."""

    meaning: str
    """What this decision means, in the deciding capability's own words. Never
    Beacon legacy desk's paraphrase."""

    def assures(self) -> bool:
        """Whether this may render without a warning affordance."""
        if self.decider is Decider.AGENT:
            return False
        if isinstance(self.binding, NoBinding):
            return False
        return self.check not in NOT_ASSURING

    def operator_action(self) -> OperatorAction:
        """What a human has to do about this, if anything.

        `force` is consulted FIRST. A spent decision needs nothing whatever it
        is attached to -- supersession *is* the resolution, and advising
        someone to re-raise a ruling the world moved past is wrong advice, not
        merely superfluous. Reading only binding and check is what made a
        superseded gate indistinguishable from a live open one.
        """
        if self.force in (Force.SPENT, Force.PENDING):
            return OperatorAction.NONE
        if self.force is Force.INDETERMINATE:
            return OperatorAction.DECIDE
        if self.check is Check.DRIFTED:
            return (
                OperatorAction.ACT
                if self.enforcement is Enforcement.REPORTED
                else OperatorAction.NONE
            )
        if isinstance(self.binding, NoBinding) or self.check is Check.UNVERIFIABLE:
            return OperatorAction.DECIDE
        return OperatorAction.NONE
