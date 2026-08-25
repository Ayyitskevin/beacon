"""Who owns each closed vocabulary, and where its canonical definition lives.

Named `ownership` rather than `vocabulary` because claude-09 owns the
vocabularies themselves (`store/vocabulary.py`, `store/enums.py`,
`store/envelope.py`). This module owns only the question *whose definition
is canonical for each table*, which is what makes reconciliation possible.
Both files were created independently with the same name; ceding it to the
lane that owns the concept is the smaller change.

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
    # Lane C (this lane) — concepts the envelope does not model.
    Vocabulary("enum_derivation_kind", "claude-2f", "mosaic.store.ownership", "DerivationKind"),
    Vocabulary("enum_replay_outcome", "claude-2f", "mosaic.store.ownership", "ReplayOutcome"),
    Vocabulary("enum_instant_encoding", "claude-2f", "mosaic.store.ownership", "InstantEncoding"),
    Vocabulary(
        "enum_digest_algorithm",
        "claude-2f",
        "mosaic.store.ownership",
        "DigestAlgorithm",
        note="Closes binding_digest.algorithm, which schema.sql leaves as any "
        "non-empty string. The difference between recording an algorithm "
        "and recording a typo.",
    ),
    # Lane A (claude-09) — the envelope's vocabularies. Registered here so the
    # reconciler covers the whole store, but the DEFINITIONS are theirs and are
    # read from their modules. This lane never holds a copy of their values.
    Vocabulary("enum_resolution", "claude-09", "mosaic.store.envelope", "Resolution"),
    Vocabulary("enum_capability", "claude-09", "mosaic.store.envelope", "Capability"),
    Vocabulary("enum_origin", "claude-09", "mosaic.store.envelope", "Origin"),
    Vocabulary("enum_relation_kind", "claude-09", "mosaic.store.envelope", "RelationKind"),
    Vocabulary("enum_binding_kind", "claude-09", "mosaic.store.enums", "BindingKind"),
    Vocabulary("enum_capability_state", "claude-09", "mosaic.store.enums", "CapabilityState"),
    Vocabulary("enum_authority_state", "claude-09", "mosaic.store.enums", "AuthorityState"),
    Vocabulary("enum_decider", "claude-09", "mosaic.store.authority", "Decider"),
    Vocabulary("enum_check_result", "claude-09", "mosaic.store.authority", "Check"),
    Vocabulary("enum_enforcement", "claude-09", "mosaic.store.authority", "Enforcement"),
    Vocabulary("enum_lifecycle", "claude-09", "mosaic.store.authority", "Lifecycle"),
    Vocabulary("enum_force", "claude-09", "mosaic.store.authority", "Force"),
    Vocabulary("enum_type_nature", "claude-09", "mosaic.store.vocabulary", "TypeNature"),
    Vocabulary("enum_status_polarity", "claude-09", "mosaic.store.vocabulary", "StatusPolarity"),
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


class DigestAlgorithm(Enum):
    ICARUS_STABLE_JSON_V1 = "icarus.stableJson.v1"
    MOSAIC_CANONICAL_V1 = "mosaic.canonical.v1"


class Resolution(Enum):
    """What the reconciler found for one vocabulary. Four states, not two."""

    MATCHED = "matched"
    DIVERGED = "diverged"
    PENDING = "pending"  # owner's module has not landed — real, not a skip
    UNOWNED = "unowned"  # a lookup table with no registry entry


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


#: Lane C's lookup tables and the definition each is seeded from. Lane A's
#: `seed_enums` does the same for theirs. Seeding from the definition is
#: strictly better than the reconcile-two-hand-lists approach this module was
#: originally built for: there is no second copy, so there is no drift to
#: detect. See `reconcile.py` for what the reconciler is still good for.
SEEDED: tuple[tuple[str, type], ...] = (
    ("enum_instant_encoding", InstantEncoding),
    ("enum_digest_algorithm", DigestAlgorithm),
    ("enum_derivation_kind", DerivationKind),
    ("enum_replay_outcome", ReplayOutcome),
)


def seed(conn) -> None:
    """Populate this lane's lookup tables from their canonical definitions."""
    for table, definition in SEEDED:
        conn.executemany(
            f"INSERT OR IGNORE INTO {table}(value) VALUES (?)",
            [(member.value,) for member in definition],
        )
