"""Totality check 2: nothing in the schema is unclaimed, in either direction.

SQL enforces check 3 by itself — an unknown enum value cannot be stored. It
cannot enforce this one, because a schema is by definition satisfied by its own
contents. So:

* **A lookup table with no registered owner** is an unowned vocabulary. Nobody
  maintains it, and the value that eventually breaks something will have been
  added by whoever needed it that afternoon.
* **A value in SQL that the owner's definition dropped** is the dangerous
  direction: the schema keeps accepting a value no reader handles. It lands, no
  constraint fires, nothing looks at it.
* **A value in the owner's definition that SQL rejects** is the loud direction —
  an insert fails — but it still fails at runtime rather than in CI.

Both directions are reported. A one-directional reconciler would pass the
entire time the schema was quietly the more permissive of the two, which is
precisely the state that produces a silent data sink.
"""

from __future__ import annotations

from dataclasses import dataclass
import sqlite3

from .schema import enum_values, schema_columns
from . import ownership
from .ownership import Resolution, Vocabulary, canonical_values


@dataclass(frozen=True)
class VocabularyReport:
    table: str
    owner: str
    resolution: Resolution
    only_in_sql: frozenset[str] = frozenset()
    only_in_definition: frozenset[str] = frozenset()

    def describe(self) -> str:
        if self.resolution is Resolution.MATCHED:
            return f"{self.table}: matches {self.owner}'s definition"
        if self.resolution is Resolution.PENDING:
            return (
                f"{self.table}: PENDING — {self.owner}'s definition has not landed. "
                f"Not a pass; nothing has been reconciled."
            )
        if self.resolution is Resolution.UNOWNED:
            return (
                f"{self.table}: UNOWNED — no registry entry. Every closed "
                f"vocabulary needs exactly one owning lane."
            )
        parts = [f"{self.table}: DIVERGED from {self.owner}'s definition"]
        if self.only_in_sql:
            parts.append(
                f"  in SQL, not in the definition: {sorted(self.only_in_sql)} "
                f"— the schema accepts values no reader handles"
            )
        if self.only_in_definition:
            parts.append(
                f"  in the definition, not in SQL: {sorted(self.only_in_definition)} "
                f"— inserts will fail at runtime"
            )
        return "\n".join(parts)


def _lookup_tables(conn: sqlite3.Connection) -> set[str]:
    """Tables whose only column is `name` plus optional descriptive columns.

    A lookup table is identified structurally rather than by a naming
    convention: it has a `name` primary key and is referenced by foreign keys.
    Naming conventions are a convention living in the reader's head.
    """
    found = set()
    for table in schema_columns(conn):
        pk = [row for row in conn.execute(f"PRAGMA table_info({table})") if row[5]]
        # Lane A spells the key column `value`; this lane originally used
        # `name`. Both are accepted rather than one being imposed — a detector
        # that only knew one convention would silently skip half the store's
        # vocabularies and report a clean reconciliation over nothing.
        if len(pk) == 1 and pk[0][1] in {"name", "value"}:
            found.add(table)
    return found


def reconcile_vocabularies(conn: sqlite3.Connection) -> list[VocabularyReport]:
    """Compare every SQL lookup table against its owner's canonical definition."""
    # Read through the module, not a name bound at import: a registry
    # captured at import time can never be swapped, so a caller that
    # changes it is silently ignored rather than obeyed.
    registered: dict[str, Vocabulary] = {v.table: v for v in ownership.REGISTRY}
    reports: list[VocabularyReport] = []

    for table in sorted(_lookup_tables(conn)):
        vocab = registered.get(table)
        if vocab is None:
            reports.append(VocabularyReport(table, "(nobody)", Resolution.UNOWNED))
            continue

        defined = canonical_values(vocab)
        if defined is None:
            reports.append(VocabularyReport(table, vocab.owner, Resolution.PENDING))
            continue

        in_sql = enum_values(conn, table)
        only_sql = in_sql - defined
        only_def = defined - in_sql
        resolution = Resolution.MATCHED if not (only_sql or only_def) else Resolution.DIVERGED
        reports.append(
            VocabularyReport(
                table, vocab.owner, resolution, frozenset(only_sql), frozenset(only_def)
            )
        )

    # A registry entry naming a table that does not exist is its own defect:
    # the reconciler would otherwise silently check nothing for that entry.
    for table, vocab in sorted(registered.items()):
        if table not in _lookup_tables(conn):
            reports.append(
                VocabularyReport(table, vocab.owner, Resolution.UNOWNED)
            )
    return reports


def reconcile_columns(
    conn: sqlite3.Connection, models: dict[str, set[str]]
) -> dict[str, tuple[frozenset[str], frozenset[str]]]:
    """Compare schema columns against the fields the models actually read.

    Returns table -> (columns no model claims, fields no column backs). The
    first is a silent data sink: writes land and nothing ever reads them. The
    second fails loudly at query time, but still at runtime rather than in CI.
    """
    out = {}
    for table, columns in schema_columns(conn).items():
        claimed = models.get(table, set())
        unclaimed = columns - claimed
        unbacked = claimed - columns
        if unclaimed or unbacked:
            out[table] = (frozenset(unclaimed), frozenset(unbacked))
    return out
