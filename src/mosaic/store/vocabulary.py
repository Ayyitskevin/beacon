"""Closed vocabularies, and the rule that keeps them closed.

Two decisions from the data-model design (Kevin ruled the family on
2026-08-25), both of which exist to stop one specific bug:

1. **Per-type closed vocabularies**, not one shared enum and not free strings.
   A shared enum was the original collapse -- Janus's `withdrawn` / `expired`
   / `superseded` all landing on `proposed`, so 4 of 13 live gates rendered
   identically to a live open gate. A free string is the opposite failure:
   under federation "pass the module's own word through" was right because the
   word belonged to someone else, but in a store we own it buys no invariants
   and anything can be spelled anything.

2. **Every lifecycle-bearing type must declare a terminal-negative member**,
   and event types must declare that they have none rather than carrying a
   dead one. A column whose value is always the same carries no information
   while implying there is something to check, and it leaves an open
   invitation: the next person adds "failed" and a lifecycle has been smuggled
   into an event type with nobody deciding.

The `refuse unknown variants` rule at the bottom is totality check 3 -- the
one neither field-level round-tripping nor schema reconciliation can see, and
the one that turns a closed vocabulary back into an open one at the worst
possible moment.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum


class TypeNature(StrEnum):
    """Whether a record type has a lifecycle at all."""

    LIFECYCLE = "lifecycle"
    """Its records move through states and can reach a terminal one. MUST
    declare a terminal-negative member."""

    EVENT = "event"
    """Its records are facts that happened. There is nothing for them to *be*
    other than to have occurred, so they carry NO status column at all.

    Measured against Vulcan's raw ledger: 48 lines, one uniform key set
    (completion_tokens, model, prompt_tokens, provider, seat, ts), no
    status-like field anywhere. The data has no status because the concept
    does not apply -- not because nobody got around to it."""


class VocabularyError(Exception):
    """A vocabulary violates the rules that make it safe to store."""


@dataclass(frozen=True, slots=True)
class Vocabulary:
    """The closed set of status words for one `(capability, kind)`.

    Constructed through `lifecycle()` or `event()`, which enforce the rules
    at construction time rather than leaving them to review.
    """

    capability: str
    kind: str
    nature: TypeNature
    members: tuple[str, ...]
    terminal_negative: tuple[str, ...]
    """Members meaning "this is over and did not end favourably" -- refused,
    withdrawn, expired, superseded. Empty for EVENT types, non-empty and
    required for LIFECYCLE types."""

    def __post_init__(self) -> None:
        if self.nature is TypeNature.EVENT:
            if self.members or self.terminal_negative:
                raise VocabularyError(
                    f"{self.capability}.{self.kind} is an event type and must "
                    "declare no status members at all. A one-member vocabulary "
                    "carries no information while implying there is something "
                    "to check, and invites a lifecycle being smuggled in later."
                )
            return
        if not self.members:
            raise VocabularyError(f"{self.capability}.{self.kind} has no members")
        if not self.terminal_negative:
            raise VocabularyError(
                f"{self.capability}.{self.kind} is a lifecycle type with no "
                "terminal-negative member. The mandated five-value vocabulary "
                "had none, so every capability with a terminal state had to "
                "fake one -- which is how superseded gates rendered as live. "
                "A store that owns its vocabularies has no excuse to repeat it."
            )
        unknown = set(self.terminal_negative) - set(self.members)
        if unknown:
            raise VocabularyError(
                f"{self.capability}.{self.kind} marks {sorted(unknown)} terminal "
                "but they are not members"
            )

    def accepts(self, word: str) -> bool:
        return word in self.members

    def is_terminal_negative(self, word: str) -> bool:
        return word in self.terminal_negative

    def refuse_unknown(self, word: str) -> str:
        """Totality check 3: refuse an unknown variant, never default it.

        Neither a field round-trip nor a schema reconciliation can see this
        one -- the field exists, the column exists, and a defaulting reader
        silently maps a value it does not know onto one it does. That is how
        a closed vocabulary becomes open again, and how a new terminal state
        gets read as a live one.
        """
        if word not in self.members:
            raise VocabularyError(
                f"{self.capability}.{self.kind}: unknown status {word!r}. "
                f"Known: {sorted(self.members)}. A reader must refuse an "
                "unknown variant rather than defaulting it -- see UNMAPPED_LEGACY "
                "for the migration path."
            )
        return word


#: The explicit state for a legacy status word with no member in our closed
#: vocabulary. Carries the original string rather than coercing to the nearest
#: member: coercion is how a terminal state becomes a live one during a
#: migration nobody re-reads. Never a null, never a silent nearest-match.
UNMAPPED_LEGACY = "unmapped_legacy"


def lifecycle(
    capability: str, kind: str, members: tuple[str, ...], *, terminal_negative: tuple[str, ...]
) -> Vocabulary:
    """Declare a closed vocabulary for a type that has states."""
    return Vocabulary(capability, kind, TypeNature.LIFECYCLE, members, terminal_negative)


def event(capability: str, kind: str) -> Vocabulary:
    """Declare a type whose records are facts that happened, with no status."""
    return Vocabulary(capability, kind, TypeNature.EVENT, (), ())
