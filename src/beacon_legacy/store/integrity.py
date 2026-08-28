"""Per-type integrity rules that must be RUN rather than held by the schema.

Some invariants cannot be constraints. "Every `evidence.claim` has a
`contains` relation" is one: a relation references a record, so the record
exists first, and any write-time CHECK would fire before the relation it
demands could exist. SQLite defers foreign keys, not arbitrary CHECKs.

So the guarantee is split, and the split is stated rather than papered over:

* **On delete and on retype** — enforced by triggers (`0002_containment.sql`).
  Those mutations are fully visible to the database, and they orphan a claim
  permanently.
* **On create** — checked by `orphaned_claims()` below. A claim can be briefly
  uncontained *inside* a transaction, which is legitimate, so the check runs
  at a boundary the caller chooses: CI, a migration's end, `beacon-legacy-desk doctor`.

Reporting the split honestly matters more than it looks. This repo already
shipped an AST suite claiming to prove "no path exists" when it proved
something weaker, and the overclaim survived three sessions. A rule described
as enforced when it is merely checked is the same defect: someone stops
looking.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass


@dataclass(frozen=True)
class Orphan:
    """A record of a type that must belong somewhere, which does not."""

    record_id: str
    capability: str
    kind: str
    title: str

    def describe(self) -> str:
        return (
            f"{self.capability}.{self.kind} {self.record_id} ({self.title!r}) "
            f"has no contains relation"
        )


#: (capability, kind) pairs that must have a container. Data rather than a
#: hardcoded WHERE clause, so adding a type is one line and the list of types
#: bearing this rule is readable in one place.
MUST_BE_CONTAINED: tuple[tuple[str, str], ...] = (("evidence", "claim"),)


def orphaned_claims(conn: sqlite3.Connection) -> list[Orphan]:
    """Records that must belong somewhere and do not.

    An empty list means the rule holds *for the types in MUST_BE_CONTAINED*.
    It does not mean the store has no orphans of other kinds — a distinction
    worth keeping, because "no violations found" and "nothing was checked" are
    the same output otherwise.
    """
    orphans: list[Orphan] = []
    for capability, kind in MUST_BE_CONTAINED:
        rows = conn.execute(
            """
            SELECT r.id, r.capability, r.kind, r.title
            FROM record r
            WHERE r.capability = ? AND r.kind = ?
              AND NOT EXISTS (
                  SELECT 1 FROM relation
                  WHERE from_record = r.id AND kind = 'contains'
              )
            ORDER BY r.id
            """,
            (capability, kind),
        )
        orphans.extend(Orphan(*row) for row in rows)
    return orphans


def contained_types_present(conn: sqlite3.Connection) -> dict[tuple[str, str], int]:
    """How many records of each contained type exist.

    The companion to `orphaned_claims`: zero orphans over zero records is not
    evidence the rule works. A caller that wants to claim the invariant holds
    needs both numbers.
    """
    counts = {}
    for capability, kind in MUST_BE_CONTAINED:
        (count,) = conn.execute(
            "SELECT count(*) FROM record WHERE capability = ? AND kind = ?",
            (capability, kind),
        ).fetchone()
        counts[(capability, kind)] = count
    return counts
