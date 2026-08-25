"""Turning one entry's authority into something safe to display.

This module used to own a five-value label vocabulary and a boolean,
`is_authority`, that answered "does this count as approved?". Both are gone.
`docs/authority.md` re-derived the fleet's approval mechanisms from live code
and showed the premise was wrong: Icarus binds a wider digest than Janus *and
enforces it on read*, while Janus deliberately does not enforce so that it
stays out of the permission path. Ranking on "binds to bytes" put those two in
the same bucket and separated Athena from Buzz for what turned out to be a
deliberate safety property in Athena's favour.

So the vocabulary is imported, never redefined here, and this module renders
four orthogonal axes without summarising them:

    decider      who made the decision
    attachment   what it is pinned to, and what checking it just produced
    enforcement  who acts when the binding stops holding
    lifecycle    where the thing is in its journey

The operationally important pair is `enforcement` x `attachment`, and the
contract answers it: `Authority.operator_action()` returns ACT for the one
combination where the system is quietly wrong and nothing is stopping anyone
(`reported` + `bytes-drifted`), DECIDE for a standing limitation nobody can
re-check (`unbound`, `bytes-unverifiable`), and NONE otherwise.

This module computes no policy of its own. Every judgement below is the
contract's, reconstructed from the document and asked directly, so the
renderer and the contract cannot drift apart.
"""

from __future__ import annotations

from dataclasses import dataclass

from mosaic.contracts.authority import (
    Attachment,
    Authority,
    Decider,
    Enforcement,
    Lifecycle,
    OperatorAction,
)

#: Words that must never appear as a rendered authority state. "Approved" is
#: the collapse itself: the one word that reads as authority while saying
#: nothing about which mechanism produced it, what it is pinned to, or whether
#: anything would stop you if it stopped holding.
FORBIDDEN_RENDERINGS: frozenset[str] = frozenset({"approved", "approval", "ok to ship"})

#: Left rule and glyph by operator action. The visual weight tracks WHAT A
#: HUMAN MUST DO, and is deliberately anti-correlated with authority strength:
#: a Janus ruling with a broken binding is loud, and an Icarus plan approval
#: that correctly refused is quiet. Anything that made the strongest mechanism
#: the loudest would be the banned ranking wearing a stylesheet.
_ACTION_STYLE: dict[OperatorAction, tuple[str, str]] = {
    OperatorAction.ACT: ("║", "⚑"),
    OperatorAction.DECIDE: ("┆", "◈"),
    OperatorAction.NONE: ("│", "·"),
}

_ATTACHMENT_GLOSS: dict[Attachment, str] = {
    Attachment.BYTES_VERIFIED: "re-derived just now, matched",
    Attachment.BYTES_DRIFTED: "re-derived, DID NOT match",
    Attachment.BYTES_UNVERIFIABLE: "can never be re-checked by anyone",
    Attachment.BYTES_UNCHECKED: "not checked this run",
    Attachment.UNBOUND: "pinned to nothing at all",
    Attachment.INTENT: "an action against a target, not its content",
    Attachment.HANDLE: "a token or record id",
    Attachment.STEP: "a position in a run",
}

_ENFORCEMENT_GLOSS: dict[Enforcement, str] = {
    Enforcement.ENFORCED: "the module refuses to proceed on drift",
    Enforcement.REPORTED: "the module reports drift but does not stop anyone",
    Enforcement.NONE: "binds no content, so nothing can drift",
}


class UnreadableAuthority(Exception):
    """An entry's authority block could not be read as the contract's shape."""


@dataclass(frozen=True)
class AuthorityView:
    """One entry's authority, ready to display, with nothing summarised.

    There is deliberately no `is_authority`, no score, and no ordering. Any
    code reducing this to one boolean is the thing the contract and this
    renderer exist to prevent.
    """

    authority: Authority | None
    """The reconstructed contract object, or None when the entry's authority
    block was unreadable."""

    rule: str
    glyph: str
    headline: str
    axes: tuple[str, ...]
    action: OperatorAction
    assures: bool
    problem: str | None = None
    """Set when the authority block could not be read. Reported, never guessed
    into a known value."""


def _member(enum_cls, raw, field: str):
    try:
        return enum_cls(str(raw))
    except ValueError as exc:
        raise UnreadableAuthority(
            f"{field}={raw!r} is not one of {[m.value for m in enum_cls]}"
        ) from exc


def read_authority(entry: dict) -> Authority | None:
    """Rebuild the contract's `Authority` from a serialized desk entry.

    Returns None when the entry carries no approval at all. That is the common
    case — most desk entries are work, evidence or conversation, and nothing
    ever ruled on them — and it is emphatically NOT a failed check. Collapsing
    "no approval was ever sought" into "an approval could not be verified" is
    the same absent-versus-broken confusion this renderer exists to prevent,
    and it would fire on the majority of a real desk.

    `compose.to_document` emits the desk through `dataclasses.asdict`, so the
    authority block arrives as a nested mapping whose values are `StrEnum`
    members before a JSON round-trip and plain strings after one. Both are
    accepted; neither is guessed at.

    Raises:
        UnreadableAuthority: if the block is missing, is not a mapping, or
            carries a value outside the contract's vocabulary. Never falls back
            to a default — inventing an authority value is strictly worse than
            reporting that one could not be read.
    """
    raw = entry.get("authority")
    if raw is None:
        return None
    if not isinstance(raw, dict):
        raise UnreadableAuthority(f"authority is a {type(raw).__name__}, not an object")

    missing = {"decider", "attachment", "enforcement", "lifecycle"} - set(raw)
    if missing:
        raise UnreadableAuthority(f"authority is missing {sorted(missing)}")

    return Authority(
        decider=_member(Decider, raw["decider"], "decider"),
        attachment=_member(Attachment, raw["attachment"], "attachment"),
        enforcement=_member(Enforcement, raw["enforcement"], "enforcement"),
        lifecycle=_member(Lifecycle, raw["lifecycle"], "lifecycle"),
        mechanism=str(raw.get("mechanism") or "unstated mechanism"),
        meaning=str(raw.get("meaning") or ""),
    )


def _headline(authority: Authority) -> str:
    """The one line above the axes. Never a bare verdict word.

    The lifecycle value is always paired with its decider, because `authorized`
    alone is the collapse: it says a decision happened without saying who made
    it or what it is pinned to.
    """
    lifecycle = authority.lifecycle.value
    # Displayed spelling only. Nothing in code depends on this string, and the
    # contract's value stays neutral until Kevin rules on the rename (gates
    # gf430f95f410 / gda8569d0227).
    if (
        authority.lifecycle is Lifecycle.AUTHORIZED
        and authority.decider is Decider.HUMAN
        and authority.mechanism.startswith("janus.")
    ):
        lifecycle = "janus-authorized"
    return f"{lifecycle} · decided by {authority.decider.value}"


def view(entry: dict) -> AuthorityView:
    """Resolve one desk entry into a display-safe view of its authority.

    Three outcomes, deliberately distinct:

    * **No approval** — nothing ever ruled on this entry. The common case.
      Renders plainly, needs nothing from anyone, and must never look like a
      check that failed.
    * **An approval** — rendered across all four axes, with the operator
      action the contract computes.
    * **An unreadable approval block** — reported as unreadable and sorted as
      needing a human decision, because an entry Mosaic cannot describe is
      exactly the sort of thing a person should look at. Never guessed into a
      known value.

    Never raises.
    """
    try:
        authority = read_authority(entry)
    except UnreadableAuthority as exc:
        rule, _ = _ACTION_STYLE[OperatorAction.DECIDE]
        return AuthorityView(
            authority=None,
            rule=rule,
            glyph="?",
            headline="authority block unreadable",
            axes=(f"! {exc}",),
            action=OperatorAction.DECIDE,
            assures=False,
            problem=str(exc),
        )

    if authority is None:
        rule, glyph = _ACTION_STYLE[OperatorAction.NONE]
        return AuthorityView(
            authority=None,
            rule=rule,
            glyph=glyph,
            headline="",
            axes=(),
            action=OperatorAction.NONE,
            assures=True,
            problem=None,
        )

    action = authority.operator_action()
    rule, glyph = _ACTION_STYLE[action]

    axes = (
        f"attached to: {authority.attachment.value} ({_ATTACHMENT_GLOSS[authority.attachment]})",
        f"on drift:    {authority.enforcement.value} ({_ENFORCEMENT_GLOSS[authority.enforcement]})",
        f"mechanism:   {authority.mechanism}",
    )

    return AuthorityView(
        authority=authority,
        rule=rule,
        glyph=glyph,
        headline=_headline(authority),
        axes=axes,
        action=action,
        assures=authority.assures(),
    )


def has_approval(entry: dict) -> bool:
    """Whether this entry carries an approval at all, readable or not."""
    return entry.get("authority") is not None


#: Worklist order: what a human must do, hardest first. This is NOT an
#: authority ranking and must never be labelled as importance or priority —
#: see docs/authority.md. A reader who learns that top-means-important will
#: conclude that top-means-trustworthy, which rebuilds the binary in the one
#: place nobody thinks to look for it.
WORKLIST_ORDER: dict[OperatorAction, int] = {
    OperatorAction.ACT: 0,
    OperatorAction.DECIDE: 1,
    OperatorAction.NONE: 2,
}


def worklist_key(entry: dict) -> tuple[int, str]:
    """Sort key for the worklist view: action first, then most recent.

    Recency breaks ties so that no secondary sort can smuggle an authority
    ordering into the tiebreak.
    """
    resolved = view(entry)
    return (WORKLIST_ORDER[resolved.action], _negated_timestamp(entry))


def _negated_timestamp(entry: dict) -> str:
    # ISO-8601 sorts lexicographically; invert each character's ordinal so a
    # plain ascending sort puts the most recent first without a second pass.
    stamp = str(entry.get("timestamp") or "")
    return "".join(chr(0x10FFFF - ord(c)) if ord(c) < 0x10FFFF else c for c in stamp)
