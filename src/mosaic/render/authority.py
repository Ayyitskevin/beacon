"""Resolving an entry's authority label into something safe to display.

This module is the executable form of Mosaic's governing rule:

    The output must distinguish proposed / reviewed / janus-authorized /
    executed / verified, and must NEVER collapse them into a generic
    "approved".

The rule exists because AI Town has *five* legitimate approval mechanisms
(Buzz ``KIND_APPROVAL_GRANT``, Buzz workflow approval, Athena action
approvals, Icarus ``approval_state``/``policy_digest``, and a Janus ruling)
and only one of them — the Janus ruling — binds to exact bytes and can be
re-checked afterwards. The danger was never that someone wires a reaction to
an approval. It is that five legitimate approvals become *visually
interchangeable*, and an operator can no longer tell which one carries
authority. See ``docs/render.md``.

So the resolver holds one line, in one place, for every renderer:

* Only Janus can render as authority.
* Janus renders as authority only while its binding is known to still hold.
* Everything else renders as a module-local fact wearing its owner's badge.
* An unrecognised label is reported as unrecognised — never downgraded into
  a generic affordance, which is exactly the collapse this rule forbids.

Renderers consume a plain ``desk.v1`` mapping, not the contract classes. That
is deliberate: the display layer must depend on the *document*, so any shell
that later renders the document inherits these guarantees for free.
"""

from __future__ import annotations

from dataclasses import dataclass

#: The five authority labels, in lifecycle order. This tuple is the whole
#: vocabulary; anything else is unrecognised, not a synonym.
AUTHORITY_LABELS: tuple[str, ...] = (
    "proposed",
    "reviewed",
    "janus-authorized",
    "executed",
    "verified",
)

#: The module that owns rulings. Only an entry sourced from here can ever
#: resolve to authority, no matter what label it carries.
AUTHORITY_SOURCE = "janus"

#: Words a rendered authority line must never contain. "Approved" is the
#: collapse itself: it is the one word that reads as authority while saying
#: nothing about which of the five mechanisms produced it. ``tests/cli``
#: asserts no renderer emits these.
FORBIDDEN_RENDERINGS: frozenset[str] = frozenset({"approved", "approval", "ok to ship"})

_BADGES: dict[str, str] = {
    "proposed": "○",
    "reviewed": "◐",
    "janus-authorized": "✔",
    "executed": "▶",
    "verified": "◆",
}


@dataclass(frozen=True)
class AuthorityVerdict:
    """How one desk entry is allowed to present itself.

    Attributes:
        label: The raw authority label carried by the entry, verbatim.
        badge: A single glyph for the label, or "?" when unrecognised.
        display: The text a renderer prints. Never a generic affordance.
        is_authority: True only for a Janus ruling whose binding is known to
            still hold. Every other combination is False — including a Janus
            ruling whose binding was never checked, because an unchecked
            binding is a claim about the past, not a fact about now.
        rechecked: Whether the binding was actually re-verified. ``None``
            means nobody checked, which is distinct from "checked, broken".
        note: Why this is not authority, when it is not. ``None`` when the
            display line already says everything.
    """

    label: str
    badge: str
    display: str
    is_authority: bool
    rechecked: bool | None
    note: str | None = None


def binding_kind(entry: dict) -> str | None:
    """Return the kind of thing a Janus ruling is bound to, if stated.

    The adapter passes Janus's own ``binding`` through verbatim as
    ``{kind, locator, sha256}``. A flat ``binding_kind`` is accepted as well,
    because a caller assembling a document by hand should not have to nest one
    field. Returns None when nothing said.

    ``"none"`` is a real, common value — on the live ledger it is 4 of 13
    gates — and it is not the same as "not stated". It means Janus recorded a
    ruling bound to nothing at all.
    """
    binding = entry.get("binding")
    if isinstance(binding, dict) and binding.get("kind"):
        return str(binding["kind"]).strip().lower()
    flat = entry.get("binding_kind")
    return str(flat).strip().lower() if flat else None


def resolve(entry: dict) -> AuthorityVerdict:
    """Resolve one ``desk.v1`` entry into a display-safe authority verdict.

    Args:
        entry: A ``desk.v1`` entry mapping. Reads ``authority`` (the label),
            ``source`` (owning module), and — for Janus entries only —
            ``binding_holds``, a tri-state: True (re-checked, still binds),
            False (re-checked, no longer binds), or absent/None (not checked).

    Returns:
        An :class:`AuthorityVerdict`. This function never raises on a
        malformed label; it reports the malformation, because a renderer that
        crashes on one bad entry hides the other forty.
    """
    label = str(entry.get("authority") or "").strip().lower()
    source = str(entry.get("source") or "").strip().lower()

    if label not in AUTHORITY_LABELS:
        shown = label or "(missing)"
        return AuthorityVerdict(
            label=label,
            badge="?",
            display=f"unrecognised authority label {shown!r}",
            is_authority=False,
            rechecked=None,
            note=(
                "Not one of the five known labels. Reported rather than "
                "guessed — mapping an unknown label onto a known one is the "
                "collapse this resolver exists to prevent."
            ),
        )

    badge = _BADGES[label]

    if label != "janus-authorized":
        # A module-local fact. It is true, it is useful, and it is not
        # authority — so it renders wearing its owner's badge and nothing more.
        return AuthorityVerdict(
            label=label,
            badge=badge,
            display=label,
            is_authority=False,
            rechecked=None,
        )

    if source != AUTHORITY_SOURCE:
        # Some other module claims a Janus ruling. It may well be telling the
        # truth, but it cannot be re-checked from here, so it renders as a
        # claim about Janus rather than as a ruling by Janus.
        return AuthorityVerdict(
            label=label,
            badge="?",
            display=f"claims janus-authorized (unverifiable: sourced from {source or 'unknown'})",
            is_authority=False,
            rechecked=None,
            note=(
                "Only an entry sourced from Janus can render as authority. "
                "Follow the canonical link and re-check the ruling there."
            ),
        )

    # Checked before binding_holds deliberately. A ruling bound to nothing
    # cannot be re-checked against anything, ever — so no field claiming the
    # binding "holds" can make it authority. The conservative order is the
    # correct one: the absence of something to check beats any claim about
    # having checked it.
    if binding_kind(entry) == "none":
        return AuthorityVerdict(
            label=label,
            badge="⊘",
            display="janus-authorized (bound to nothing — no binding to re-check, ever)",
            is_authority=False,
            rechecked=False,
            note=(
                "A human ruled, but the ruling was recorded against no "
                "artifact. It cannot be shown to still cover anything. This is "
                "a standing condition, not a gap someone can close by looking."
            ),
        )

    binding_holds = entry.get("binding_holds")

    if binding_holds is True:
        # The only path to authority in the whole system.
        return AuthorityVerdict(
            label=label,
            badge=badge,
            display="janus-authorized (binding re-checked, still holds)",
            is_authority=True,
            rechecked=True,
        )

    if binding_holds is False:
        return AuthorityVerdict(
            label=label,
            badge="⚠",
            display="AUTHORIZATION STALE — a human ruled, but the bytes changed since",
            is_authority=False,
            rechecked=True,
            note=(
                "A Janus ruling binds exact bytes. The binding no longer "
                "holds, so the ruling does not cover what is there now."
            ),
        )

    return AuthorityVerdict(
        label=label,
        badge="?",
        display="janus-authorized (binding NOT re-checked)",
        is_authority=False,
        rechecked=None,
        note=(
            "Janus recorded a ruling against something, but nothing "
            "re-verified the binding for this view. Unlike a ruling bound to "
            "nothing, this one can be settled — re-check it in Janus."
        ),
    )
