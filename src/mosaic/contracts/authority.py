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
    """An agent proposed or self-attested. NEVER renders as an approval."""


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

    def assures(self) -> bool:
        """Whether this may render without a warning affordance.

        NOT a judgement that the thing is approved, safe, or ranked above
        another entry. One narrow question, one narrow answer.
        """
        return self.attachment not in NOT_ASSURING

    def operator_must_act(self) -> bool:
        """Whether a human has to do something for this entry to be safe.

        True in exactly one case: the binding is known broken and the owning
        module does not enforce it. Then nothing in the system is stopping
        anyone from acting on a void approval, and only a person can close
        that gap.

        Deliberately False for ENFORCED + BYTES_DRIFTED: the module already
        refused, so the operator is investigating, not containing.
        """
        return (
            self.attachment is Attachment.BYTES_DRIFTED
            and self.enforcement is Enforcement.REPORTED
        )
