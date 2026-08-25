"""Who owns each closed vocabulary, and where its canonical definition lives.

The rule (Fable, phase 2): every enum has exactly ONE owning lane whose
definition is canonical. The SQL lookup table is a *materialisation* of that
definition, not a second source. Runtime reads values from the database via
`schema.enum_values`, so no layer carries a hardcoded copy.

This registry is what makes the reconciliation test possible: it names, for
each lookup table, the owner and the import path of their canonical
definition. `tests/store/test_reconciliation.py` walks it and fails CI on any
divergence in either direction.

Why the registry rather than a list of expected values here: a list here would
BE the second copy. This file deliberately holds no vocabulary content — only
addresses.

A vocabulary whose owner has not landed yet is `pending`, which is not the
same as absent and not the same as broken. Applying this project's own
distinction to its own tooling is not a joke: the alternative is a test that
skips silently, and a silently-skipped check is indistinguishable from a
passing one.
"""

from __future__ import annotations

import importlib
from dataclasses import dataclass
from enum import Enum


@dataclass(frozen=True)
class Vocabulary:
    """One closed vocabulary: its SQL table, its owner, and where truth lives."""

    table: str
    owner: str
    module: str
    attribute: str
    note: str = ""


#: Every lookup table in the schema must appear here. An unregistered lookup
#: table is an unowned vocabulary, which the reconciliation test reports as a
#: failure rather than ignoring — the whole class of bug this guards against is
#: a value that no one owns and therefore no one maintains.
REGISTRY: tuple[Vocabulary, ...] = (
    # Lane C (this lane) — the store's own concepts.
    Vocabulary("derivation_kind", "claude-2f", "mosaic.store.vocabulary", "DerivationKind"),
    Vocabulary("replay_outcome", "claude-2f", "mosaic.store.vocabulary", "ReplayOutcome"),
    Vocabulary("instant_encoding", "claude-2f", "mosaic.store.vocabulary", "InstantEncoding"),
    Vocabulary("origin_system", "claude-2f", "mosaic.store.vocabulary", "OriginSystem"),
    Vocabulary("value_origin", "claude-2f", "mosaic.store.vocabulary", "ValueOrigin"),
    Vocabulary(
        "digest_algorithm", "claude-2f", "mosaic.store.vocabulary", "DigestAlgorithm",
        note="Rows carry frozen_at and description as well; only names reconcile.",
    ),
    # Lane A (claude-09) — the envelope's authority axes. Not landed yet.
    Vocabulary(
        "reference_state", "claude-09", "mosaic.contracts.references", "ReferenceState",
        note="Reference resolution is claude-09's; this lane materialises it only.",
    ),
)


class DerivationKind(Enum):
    CLOCK_DERIVED = "clock_derived"
    CROSS_TABLE_DERIVED = "cross_table_derived"
    ROW_DERIVED = "row_derived"


class ReplayOutcome(Enum):
    REPRODUCED = "reproduced"
    DIVERGED = "diverged"
    NOT_REPLAYABLE = "not_replayable"
    NOT_ATTEMPTED = "not_attempted"


class InstantEncoding(Enum):
    ISO8601Z = "iso8601z"
    NAIVE_UTC = "naive_utc"
    EPOCH_SECONDS = "epoch_seconds"
    EPOCH_MILLIS = "epoch_millis"


class OriginSystem(Enum):
    BUZZ = "buzz"
    ATHENA = "athena"
    MINERVA = "minerva"
    VULCAN = "vulcan"
    JANUS = "janus"
    ICARUS = "icarus"
    REPLAYDECK = "replaydeck"
    MOSAIC = "mosaic"


class ValueOrigin(Enum):
    OBSERVED = "observed"
    DERIVED = "derived"
    NATIVE = "native"


class DigestAlgorithm(Enum):
    ICARUS_STABLE_JSON_V1 = "icarus.stableJson.v1"
    MOSAIC_CANONICAL_V1 = "mosaic.canonical.v1"


class Resolution(Enum):
    """What the reconciler found for one vocabulary. Four states, not two."""

    MATCHED = "matched"
    DIVERGED = "diverged"
    PENDING = "pending"      # owner's module has not landed — real, not a skip
    UNOWNED = "unowned"      # a lookup table with no registry entry


def canonical_values(vocab: Vocabulary) -> set[str] | None:
    """The owner's definition, or None when their module has not landed.

    None means pending. It never means empty: an owner who defines an empty
    vocabulary returns an empty set, which is a different fact.
    """
    try:
        module = importlib.import_module(vocab.module)
    except ImportError:
        return None
    try:
        definition = getattr(module, vocab.attribute)
    except AttributeError:
        return None
    return {member.value for member in definition}
