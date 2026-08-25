"""Loading the store schema, and the two totality checks the SQL cannot make.

The migrations in `migrations/` enforce one of the three totality checks by
themselves: an unknown enum value cannot be stored at all, because every enum
is a lookup table and every user of it is a foreign key. The database is the
reader that refuses.

The other two checks are not expressible in SQL and live here as functions the
test suite calls:

* **A column no model claims.** A column added to the schema that no Python
  model reads is a silent data sink — writes land, nothing ever looks. SQL
  cannot detect this because the schema is, by definition, satisfied.
* **A field added to a type the reader ignores.** Measured this session: a
  renderer rebuilt a dataclass field-by-field against a six-field version and
  silently defaulted the seventh, while 198 tests passed and three lanes each
  verified their own half correctly.

Saying which checks are enforced where matters more than it looks. The
read-only AST suite in this repo claimed to prove "no path exists" when it
proved "no adapter has an I/O path of its own", and that overclaim survived
three sessions until someone executed it.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

MIGRATIONS = Path(__file__).parent / "migrations"


#: Lane A's envelope core. Applied FIRST: the integrity layer constrains its
#: tables and cannot be loaded without them. Two schema sources is deliberate
#: (shape and integrity are owned by different lanes), but only one loader
#: knows about both, and it lives here.
CORE = Path(__file__).parent / "schema.sql"


def apply_schema(conn: sqlite3.Connection) -> None:
    """Apply Lane A's envelope, then this lane's integrity migrations."""
    conn.execute("PRAGMA foreign_keys = ON")
    conn.executescript(CORE.read_text())
    for path in sorted(MIGRATIONS.glob("*.sql")):
        conn.executescript(path.read_text())

    # Both lanes seed their own vocabularies from their own definitions. A
    # loader that applied the DDL and skipped the seeding would produce a
    # schema whose every lookup table is empty and whose every foreign key
    # therefore rejects everything -- which fails loudly, but only at the first
    # insert, far from here.
    from . import ownership
    from .enums import seed_enums

    seed_enums(conn)
    ownership.seed(conn)
    conn.commit()
    conn.execute("PRAGMA foreign_keys = ON")


def schema_columns(conn: sqlite3.Connection) -> dict[str, set[str]]:
    """Every table and its columns, as the database actually has them."""
    tables = [
        row[0]
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
        )
    ]
    return {
        table: {row[1] for row in conn.execute(f"PRAGMA table_info({table})")} for table in tables
    }


def enum_values(conn: sqlite3.Connection, table: str) -> set[str]:
    """The permitted values of an enum lookup table, read from the database.

    Callers must read enums through this rather than hardcoding a copy. A
    second hardcoded list is a second source of truth, and the two drift in the
    direction nobody notices — the schema keeps accepting a value the code
    stopped handling.
    """
    columns = {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}
    key = "value" if "value" in columns else "name"
    return {row[0] for row in conn.execute(f"SELECT {key} FROM {table}")}
