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


def apply_schema(conn: sqlite3.Connection) -> None:
    """Apply every migration in filename order."""
    conn.execute("PRAGMA foreign_keys = ON")
    for path in sorted(MIGRATIONS.glob("*.sql")):
        conn.executescript(path.read_text())
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
    return {row[0] for row in conn.execute(f"SELECT name FROM {table}")}
