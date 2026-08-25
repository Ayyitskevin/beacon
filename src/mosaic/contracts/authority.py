"""Authority labels: who decided, what it is pinned to, who acts on drift.

Four orthogonal axes. Collapsing any of them is the failure this module
exists to prevent, and `docs/authority.md` carries the evidence for each.

The short version: the fleet has at least six distinct approval mechanisms,
and the intuitive ranking ("binds to bytes = stronger") is wrong. Icarus binds
a wider digest than Janus and enforces it on read; Janus deliberately reports
drift without enforcing, to stay out of the permission path; Athena
deliberately binds an intent rather than a payload, because replaying a stored
mutation would be a new trust boundary. None of those is "weaker" -- they are
answers to different questions.

So Mosaic does not rank. It renders the axes and refuses to summarize them.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum


class Decider(StrEnum):
    """Who made the decision. Not how important the mechanism is."""

    HUMAN = "human"
    """A person ruled: a Janus ruling, an Athena operator decision, a Buzz
    grant signed by a human pubkey."""

    POLICY = "policy"
    """A rule evaluated with no person in the loop at decision time."""

    AGENT = "agent"
    """An agent proposed or self-attested. NEVER renders as an approval --
    enforced by `Authority.assures()`, which returns False for AGENT whatever
    the attachment says. An agent verifying content it computed itself is the
    highest-consequence case in this file, and a docstring is not a guarantee.

    No live producer emits AGENT as of 2026-08-24. It exists so an adapter has
    a correct value to reach for rather than reaching for HUMAN."""


class Attachment(StrEnum):
    """What the decision is pinned to, and what checking it just produced.

    Note the four-way split at the bottom. `BYTES_UNVERIFIABLE` and
    `BYTES_UNCHECKED` are different facts about the world and lead to
    different operator actions, so they are different values -- rendering both
    as "not re-checked" would rebuild the collapse in miniature.
    """

    BYTES_VERIFIED = "bytes-verified"
    """Binds a content digest; Mosaic re-derived it just now and it matched.
    The only value that may render as a positive assurance."""

    BYTES_DRIFTED = "bytes-drifted"
    """Binds a content digest, re-derived, and it did NOT match. A finding.
    Renders louder than the approval it belongs to: a ruling approves specific
    bytes, it does not follow them."""

    BYTES_UNVERIFIABLE = "bytes-unverifiable"
    """Binds bytes that CAN NEVER be re-checked by anyone -- a permanent
    property of the binding, not of this run. The code path is a Janus
    `text`-kind binding, whose verify_binding returns "inline text binding --
    nothing live to re-check". No amount of connectivity fixes it, but the
    bytes are at least *shown*, so a reader can judge them.

    Measured 2026-08-24: zero of 13 live Janus gates use `text`. This is a
    real code path with no population yet."""

    UNBOUND = "unbound"
    """Attached to NOTHING. No binding was ever recorded, so there is no
    artifact to show and nothing to re-check against -- not now, not ever, by
    anyone.

    The weakest state in the system, and the easiest to miss, because it
    arrives wearing the same lifecycle value as the strongest. Measured
    2026-08-24: **4 of 13 live Janus gates are `binding_kind: null`, and all
    four are `approved`** -- sitting in the same list as 8 git-bound rulings.

    Distinct from BYTES_UNVERIFIABLE: that binds bytes it cannot re-derive;
    this binds nothing at all. Collapsing them would hide the more dangerous
    of the two."""

    BYTES_UNCHECKED = "bytes-unchecked"
    """Binds bytes Mosaic DID NOT check this run -- module unreachable, repo
    absent, offline. A transient fact about this run. Re-running with the
    module reachable may resolve it to VERIFIED or DRIFTED. Distinct from
    UNVERIFIABLE because the operator action is different: here there is
    something to go do."""

    INTENT = "intent"
    """Binds an action against a target, deliberately not its content
    (Athena). Not a weaker BYTES_VERIFIED; a different promise."""

    HANDLE = "handle"
    """Binds a token or record id (Buzz 46030)."""

    STEP = "step"
    """Binds a position in a run: workflow/run/step (Buzz 46011)."""


class Enforcement(StrEnum):
    """Who acts when the binding no longer holds. **The operator-action axis.**

    This exists because "binds to bytes" and "enforces on read" are separate
    properties, and the difference is the single most operationally important
    thing on an entry. Without it, a mechanism that binds-but-does-not-enforce
    and one that enforces-but-binds-narrowly both read as "authorized with a
    different footnote", which is the collapse in a subtler form.
    """

    ENFORCED = "enforced"
    """The owning module refuses to proceed when the binding fails. Icarus
    re-derives planApprovalDigest on read and calls sourceInvalid() on
    mismatch. **Operator action on drift: none is required to stay safe** --
    the system already stopped. Investigate why, at leisure."""

    REPORTED = "reported"
    """The owning module states the drift and stops there, on purpose. Janus:
    "enforcing it would put Janus in the permission path." **Operator action on
    drift: REQUIRED.** Nothing is preventing anyone from acting on a void
    ruling; a human must raise a new gate."""

    NONE = "none"
    """The mechanism has no drift concept at all, because it binds no content
    (Athena intent, Buzz handle/step). Not a failure -- there is simply
    nothing that could drift. Its safety comes from elsewhere: single-use
    consumption, expiry, an approver check."""


class Lifecycle(StrEnum):
    """Where the thing is in its journey. Mandated by the ORACLE roadmap.

    The roadmap's list is `proposed, reviewed, janus-authorized, executed,
    verified`. `AUTHORIZED` is spelled without the module name on purpose:
    `janus-authorized` hardcodes the claim that Janus is the sole authorizing
    mechanism, which docs/authority.md disproves. Renaming a mandated boundary
    is Kevin's call, not a lane's, so it is raised as a Janus gate -- and until
    he rules, adapters emit AUTHORIZED and populate `decider`, while a renderer
    may still *display* "janus-authorized" for Janus-decided entries. Nothing
    in code depends on that string.
    """

    PROPOSED = "proposed"
    REVIEWED = "reviewed"
    AUTHORIZED = "authorized"
    EXECUTED = "executed"
    VERIFIED = "verified"


class OperatorAction(StrEnum):
    """What a human has to do. Three-valued on purpose.

    A live breach and a standing limitation are both human work, and they are
    not the same human work: one is contained by acting, the other is closed
    by deciding. A boolean forces one of them to be silent.
    """

    ACT = "act"
    """Something that WAS verified is now broken, and nothing in the system is
    stopping anyone from relying on it. Contain it: raise a new gate."""

    DECIDE = "decide"
    """Nothing was ever re-checkable here -- no binding at all, or a binding
    nobody can re-derive. Not a breach and not urgent, but it will never
    resolve itself. Accept it as resting on the raiser's word, or re-raise
    with a checkable binding."""

    NONE = "none"
    """Nothing for a human to do about this entry right now."""


#: Attachments that can never be re-checked by anyone -- a standing property
#: of the binding, not of this run. These need a human DECISION, never resolve
#: on their own, and are the reason operator action is not a boolean.
_NEVER_CHECKABLE: frozenset[Attachment] = frozenset(
    {Attachment.UNBOUND, Attachment.BYTES_UNVERIFIABLE}
)

#: Attachments that must never render as a positive assurance, whatever the
#: lifecycle says. A drifted, uncheckable, or unchecked binding is not green.
NOT_ASSURING: frozenset[Attachment] = frozenset(
    {
        Attachment.BYTES_DRIFTED,
        Attachment.BYTES_UNVERIFIABLE,
        Attachment.BYTES_UNCHECKED,
        Attachment.UNBOUND,
    }
)


@dataclass(frozen=True, slots=True)
class Authority:
    """One approval, wearing its owner's badge.

    There is deliberately no `is_approved` property and no ordering. Any code
    reducing this to one boolean is the thing this type exists to prevent;
    that reduction is a doctrine decision (docs/authority.md, open question 1),
    not a convenience method.
    """

    decider: Decider
    attachment: Attachment
    enforcement: Enforcement
    lifecycle: Lifecycle

    mechanism: str
    """The owning module's own name for this mechanism: `"janus.ruling"`,
    `"athena.approval_request"`, `"buzz.kind.46030"`,
    `"icarus.plan_approval"`. Buzz kind numbers are recorded literally because
    Buzz is upstream OSS and can renumber without consulting AI Town -- a
    change must surface as a change, not a silent relabel."""

    meaning: str
    """What this approval means, in the OWNING MODULE's words -- not Mosaic's
    paraphrase. Convention lifted from Athena's desk, which carries a per-lane
    `meaning` (e.g. "asked of you and not yet answered"). Mosaic is not
    entitled to restate another module's semantics more briefly than the
    module does."""

    spent: bool = False
    """The owning module considers this decision closed and no longer in
    force: Janus `refused` / `withdrawn` / `expired` / `superseded`.

    **This exists because `lifecycle` cannot express it.** The mandated
    roadmap vocabulary (`proposed · reviewed · authorized · executed ·
    verified`) has no terminal-negative value, so an adapter that refuses to
    default upward must land all four of those states on REVIEWED -- where
    they are indistinguishable from a live decision someone is actively
    reviewing. Reading `lifecycle` therefore cannot answer "is this spent?",
    and a fix that tried would either silence genuine in-review entries or
    change nothing.

    So terminality arrives as its own fact, supplied by the adapter that can
    see the module's own status word. This is a **workaround for a gap in the
    vocabulary, not a fix for it** -- the gap is `docs/authority.md` open
    question 5 and remains Kevin's call. If the vocabulary ever gains a spent
    state, this field should be deleted, not kept alongside it.
    """

    def assures(self) -> bool:
        """Whether this may render without a warning affordance.

        NOT a judgement that the thing is approved, safe, or ranked above
        another entry. One narrow question, one narrow answer.

        An AGENT decision never assures, whatever it is attached to. The
        attachment says a digest matched; it says nothing about whether the
        party that computed it is entitled to approve it.
        """
        if self.decider is Decider.AGENT:
            return False
        return self.attachment not in NOT_ASSURING

    def operator_action(self) -> OperatorAction:
        """What a human has to do about this entry, if anything.

        Three-valued rather than boolean, because "something broke" and
        "nothing was ever checkable" are both human work and are not the same
        work. Flattening them is the collapse this module exists to prevent,
        one level down -- an earlier boolean version answered False for the
        four live UNBOUND gates the doctrine page is built around, which made
        the weakest state in the ledger the quietest.

        BYTES_UNCHECKED deliberately maps to NONE. It is a fact about this
        run, not about the decision, and `mosaic doctor` owns that surface.
        In a fleet where most hops are legitimately unreachable, flagging
        every unchecked entry would drown the two values that mean something.

        A `spent` decision needs nothing, whatever it is attached to.
        Supersession IS the resolution -- asking a human to re-raise a ruling
        the world has already moved past is not a superfluous prompt, it is
        wrong advice. This shipped: gate `g1a603e736cb` was `superseded` and
        still returned DECIDE, indistinguishable from a live open gate,
        because the predicate read only `attachment` and `enforcement`.
        """
        if self.spent:
            return OperatorAction.NONE
        if self.attachment is Attachment.BYTES_DRIFTED:
            # Enforced: the module already refused. The operator investigates,
            # but nothing is unsafe meanwhile.
            return (
                OperatorAction.ACT
                if self.enforcement is Enforcement.REPORTED
                else OperatorAction.NONE
            )
        if self.attachment in _NEVER_CHECKABLE:
            return OperatorAction.DECIDE
        return OperatorAction.NONE
