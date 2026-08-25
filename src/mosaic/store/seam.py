"""The consult-seam: the one place an operation consults authority.

Kevin ruled the permission model on 2026-08-25: **report by default, enforce
per-capability for irreversible operations.** This module is where that is
implemented, and it is deliberately one place.

Why a seam rather than a check in each operation: enforcement is a behaviour,
not a stored value. `enforcement = enforced` on a record makes nothing
enforce; something has to *consult* authority before proceeding. Scattering
that decision across operations means the policy lives in N places and drifts;
concentrating it here makes enforce-vs-report a configuration change rather
than a rewrite.

**The honest cost, recorded because it was sold to Kevin with it attached:**
this commits to authority being consultable on the critical path, which is
most of the architectural weight of enforcing. That was stated when the
proposal was made, not discovered afterwards.

**Fail-closed is the point.** An operation that does not declare its
reversibility is treated as irreversible and enforced. If the permissive value
were the silent default, "reversible" would become the rule nobody decided --
which is exactly how this project spent ten hours building the wrong product,
one layer up.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum

from mosaic.store.authority import Authority, Check, Enforcement, Force
from mosaic.store.envelope import Capability


class Reversibility(StrEnum):
    """Whether an operation can be undone. **A required declaration.**

    Not inferred from the operation's name, its verb, or what it touches. The
    seam reads a flag; it does not judge intent. An operation added without
    declaring this does not get a default -- see `OperationSpec`.
    """

    REVERSIBLE = "reversible"
    IRREVERSIBLE = "irreversible"


class Disposition(StrEnum):
    """What the seam decided about one attempt."""

    PROCEED = "proceed"
    """Nothing stands in the way."""

    PROCEED_REPORTING = "proceed_reporting"
    """A problem was found and this capability reports rather than enforces.
    The operation proceeds AND the finding is surfaced. **Both halves are
    required** -- proceeding silently would make the report meaningless, and
    it is the state where a human must act."""

    REFUSE = "refuse"
    """Enforced, and the check failed. The operation does not proceed."""


class SeamError(Exception):
    """The seam was used in a way that would hide a decision."""


@dataclass(frozen=True, slots=True)
class OperationSpec:
    """What an operation must declare before the seam will consider it.

    Every field is required. There is no constructor default for
    `reversibility`, so an operation that forgets it fails at construction
    rather than silently acquiring the permissive value.
    """

    name: str
    capability: Capability
    reversibility: Reversibility


@dataclass(frozen=True, slots=True)
class Decision:
    """The seam's answer, and why. Never a bare boolean.

    `reason` is not decoration: a refusal a human cannot explain is a refusal
    they will route around, and a report nobody can act on is noise.
    """

    disposition: Disposition
    reason: str
    operation: str
    enforced: bool
    """Whether this capability was in enforcing mode for this operation.
    Recorded so a PROCEED can be distinguished from a PROCEED that only
    happened because enforcement was off."""


class Policy:
    """Which capabilities enforce, for irreversible operations.

    Kevin's ruling in executable form. Enforcement is per-capability, applies
    only to operations declared irreversible, and everything else reports.

    An unlisted capability **enforces** for irreversible operations. Fail
    closed: a capability added to the product without a policy entry gets the
    safe behaviour, not the convenient one.
    """

    def __init__(self, reporting_only: frozenset[Capability] | None = None) -> None:
        self._reporting_only = reporting_only or frozenset()

    def enforces(self, spec: OperationSpec) -> bool:
        if spec.reversibility is Reversibility.REVERSIBLE:
            return False
        return spec.capability not in self._reporting_only


def consult(
    spec: OperationSpec,
    authority: Authority | None,
    *,
    policy: Policy,
    requires_authority: bool,
) -> Decision:
    """The single place an operation consults authority.

    `requires_authority` is explicit rather than inferred from whether
    `authority` happens to be None. Those are different facts: an operation
    that needs no approval and an operation whose approval is missing must not
    take the same path, and letting `None` decide would make them identical --
    the absent-versus-broken collapse, in the permission path.
    """
    enforced = policy.enforces(spec)

    if not requires_authority:
        if authority is not None:
            raise SeamError(
                f"{spec.name}: requires_authority=False but an authority was "
                "supplied. One of the two is wrong, and guessing which would "
                "hide it."
            )
        return Decision(Disposition.PROCEED, "no approval required", spec.name, enforced)

    if authority is None:
        return _verdict(enforced, "approval required and none is recorded", spec)

    if authority.force is not Force.IN_FORCE:
        return _verdict(
            enforced,
            f"the approval is not in force ({authority.force})",
            spec,
        )

    if authority.check is Check.DRIFTED:
        return _verdict(enforced, "the approval's binding no longer matches", spec)

    if authority.check in (Check.UNVERIFIABLE, Check.UNCHECKED):
        return _verdict(
            enforced,
            f"the approval could not be re-derived ({authority.check})",
            spec,
        )

    if authority.enforcement is Enforcement.ENFORCED and not enforced:
        raise SeamError(
            f"{spec.name}: the approval records enforcement=enforced but policy "
            "puts this capability in reporting mode. Proceeding either way would "
            "contradict one of them -- resolve the policy rather than picking."
        )

    return Decision(Disposition.PROCEED, "approval is in force and verified", spec.name, enforced)


def _verdict(enforced: bool, reason: str, spec: OperationSpec) -> Decision:
    """Enforce or report, per policy. The one branch Kevin's ruling controls."""
    if enforced:
        return Decision(Disposition.REFUSE, reason, spec.name, True)
    return Decision(Disposition.PROCEED_REPORTING, reason, spec.name, False)
