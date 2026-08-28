"""Loading Janus records into the unified store: the end-to-end proof.

This is the checkpoint the design phase was for. One real capability travels
the whole stack — frozen corpus, reader, vocabulary, envelope, integrity — and
every rule the phase argued for is either enforced or visibly checked.

What it demonstrates, in the order it would break:

* The reader refuses an unrecognised source value rather than defaulting it.
* Janus's own word survives into `record.origin_status`, including the three
  terminal-negative states that have no home in a forward-progress lifecycle.
* A clock-derived verdict (Janus's decay) is NOT stored; its inputs are.
* Every enum crossing the boundary is checked against the store's closed
  vocabularies, and the database refuses anything else.
"""

from __future__ import annotations

import json
import sqlite3

from ..readers import SourceRecord

#: Janus gate state -> the store's polarity for authority.gate. The mapping is
#: declared here, in one place, and the ORIGIN WORD IS STILL CARRIED — this
#: table decides how a record sorts, never what it is called.
STATE_POLARITY: dict[str, str] = {
    "open": "active",
    "approved": "terminal_positive",
    "refused": "terminal_negative",
    "withdrawn": "terminal_negative",
    "expired": "terminal_negative",
    "superseded": "terminal_negative",
}

RECORD_COLS = (
    "id, capability, kind, title, status, occurred_at, recorded_at, "
    "provenance_origin, provenance_source_ref, provenance_source_rev, "
    "provenance_actor, provenance_imported_at, body, body_unmapped, "
    "authority_state, authority_id"
)


def declare_vocabulary(conn: sqlite3.Connection, polarities: dict[str, str]) -> None:
    """Register authority.gate and its members before any record can exist.

    The store refuses a record whose (capability, kind) has no vocabulary, and
    refuses a status that is not a declared member. So this runs first, and its
    failure is loud rather than a silently-skipped import.
    """
    conn.execute(
        "INSERT OR IGNORE INTO capability_registry VALUES ('authority','built','live','2026-08-25')"
    )
    conn.execute("INSERT OR IGNORE INTO vocabulary VALUES ('authority','gate','lifecycle')")
    conn.executemany(
        "INSERT OR IGNORE INTO vocabulary_member VALUES ('authority','gate',?,?)",
        sorted(polarities.items()),
    )


def load(conn: sqlite3.Connection, records: list[SourceRecord], imported_at: str) -> int:
    """Insert reader output as store records. Returns the number written."""
    declare_vocabulary(conn, STATE_POLARITY)

    written = 0
    for record in records:
        payload = record.payload
        conn.execute(
            f"INSERT INTO record({RECORD_COLS}) VALUES ({','.join('?' * 16)})",
            (
                f"janus:{record.origin_ref}",
                "authority",
                "gate",
                payload["question"][:200] or "(no question recorded)",
                # Janus's own word, unmapped. The polarity table above decides
                # how it sorts; it never decides what it is called.
                record.origin_status,
                payload["raised_at"],
                imported_at,
                record.origin,
                record.origin_ref,
                "",
                payload["raised_by"] or "unknown",
                imported_at,
                json.dumps(payload, sort_keys=True),
                # Everything the reader carried that the typed body does not
                # model. Kept rather than dropped: after Janus retires this is
                # the only copy.
                json.dumps(record.derived_inputs, sort_keys=True),
                "not_required",
                "",
            ),
        )
        written += 1
    return written
