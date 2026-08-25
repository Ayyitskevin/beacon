"""Canonical enum definitions, and the one place they are materialized to SQL.

**One definition, two representations, no hand-maintained second copy.** The
Python enum is canonical; the SQL lookup table is generated from it by
`seed_enums`. Nothing anywhere writes a value list twice.

This replaces inline `CHECK (col IN ('a','b','c'))`, which was exactly the
defect the cross-lane rule forbids: the list in the DDL and the list in the
enum are two hand-maintained copies, and they drift silently in the direction
nothing detects -- the schema keeps accepting a value the code has dropped.

Lane A owns these definitions. Lane C's reconciliation test reads
`CANONICAL_ENUMS` and asserts the database matches it **in both directions**,
so a value added to one side and not the other fails CI rather than passing
until someone stores the wrong thing.
"""

from __future__ import annotations

import sqlite3
from enum import StrEnum

from mosaic.store.authority import Check, Decider, Enforcement, Force, Lifecycle
from mosaic.store.envelope import Capability, Origin, RelationKind, Resolution
from mosaic.store.vocabulary import TypeNature


class BindingKind(StrEnum):
    """Discriminator for the binding tagged union.

    Not derived from the variant classes at runtime: the discriminator is
    stored, and a stored value must have a definition that does not depend on
    which classes happen to be imported.
    """

    DIGEST = "digest"
    INTENT = "intent"
    HANDLE = "handle"
    STEP = "step"
    NONE = "none"


class CapabilityState(StrEnum):
    """Whether a capability exists, and if not, in what way it does not.

    Level 4 of the absence rule. `NOT_BUILT` and `DEFERRED` are both "no
    implementation" and are different facts -- reporting the first as the
    second sends someone to restart a service nobody wrote.
    """

    BUILT = "built"
    NOT_BUILT = "not_built"
    DEFERRED = "deferred"
    RETIRED = "retired"


class AuthorityState(StrEnum):
    """Whether a record carries a decision at all.

    Explicit because "no approval was ever sought" and "an approval is
    missing" are different facts, and a nullable foreign key would make them
    one.
    """

    REQUIRED = "required"
    NOT_REQUIRED = "not_required"


#: table name -> the enum that defines its legal values. **The registry Lane C
#: reconciles against.** Adding an enum to the schema without adding it here
#: means it has no canonical owner, which is the state this rule exists to
#: prevent.
CANONICAL_ENUMS: dict[str, type[StrEnum]] = {
    "enum_capability": Capability,
    "enum_origin": Origin,
    "enum_resolution": Resolution,
    "enum_relation_kind": RelationKind,
    "enum_decider": Decider,
    "enum_check_result": Check,
    "enum_enforcement": Enforcement,
    "enum_lifecycle": Lifecycle,
    "enum_force": Force,
    "enum_binding_kind": BindingKind,
    "enum_capability_state": CapabilityState,
    "enum_authority_state": AuthorityState,
    "enum_type_nature": TypeNature,
}


def seed_enums(conn: sqlite3.Connection) -> None:
    """Materialize every canonical enum into its lookup table.

    Idempotent. Must run before any row referencing an enum value is inserted,
    because the constraint is a foreign key rather than a CHECK -- which is
    the point: an FK reads its legal values from data seeded here, so there is
    no second list to maintain.
    """
    for table, enum in CANONICAL_ENUMS.items():
        conn.executemany(
            f"INSERT OR IGNORE INTO {table} (value) VALUES (?)",
            [(member.value,) for member in enum],
        )


def divergences(conn: sqlite3.Connection) -> dict[str, dict[str, list[str]]]:
    """Where the database and the canonical definitions disagree, both ways.

    Returned rather than asserted so a reconciliation test owns the failure
    message. **Both directions matter and only one is intuitive:** a value in
    the code but not the database fails loudly the first time it is stored,
    while a value in the database but not the code fails *silently* -- the
    schema goes on accepting something nothing produces any more, and a stale
    row keeps validating forever.
    """
    out: dict[str, dict[str, list[str]]] = {}
    for table, enum in CANONICAL_ENUMS.items():
        stored = {r[0] for r in conn.execute(f"SELECT value FROM {table}")}
        defined = {m.value for m in enum}
        missing = sorted(defined - stored)
        extra = sorted(stored - defined)
        if missing or extra:
            out[table] = {"in_code_not_db": missing, "in_db_not_code": extra}
    return out
