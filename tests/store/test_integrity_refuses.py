"""The integrity layer must refuse, not merely document.

Scope note. Lane A's schema.sql owns the envelope's shape and its own
invariants; this lane owns integrity ON TOP of it. So nothing here re-asserts
something schema.sql already enforces — two layers both enforcing an invariant
is how they drift. These tests cover only what this lane adds: the concepts the
envelope does not model, and the columns it deliberately leaves open.
"""

from __future__ import annotations

import sqlite3

import pytest

from beacon_legacy.store.schema import apply_schema, enum_values

RECORD_COLS = (
    "id, capability, kind, title, status, occurred_at, recorded_at, "
    "provenance_origin, provenance_source_ref, provenance_source_rev, "
    "provenance_actor, provenance_imported_at, body, body_unmapped, "
    "authority_state, authority_id"
)
#: Every one of those is NOT NULL with no default, by Lane A's design: a
#: default is a value nobody chose. So a fixture must state all of them, which
#: is the schema making its own completeness requirement visible.


@pytest.fixture
def db():
    conn = sqlite3.connect(":memory:")
    apply_schema(conn)
    # Lane A's prerequisites for a record: the capability must be registered
    # and the (capability, kind) pair must exist in the vocabulary table. That
    # a record cannot exist without them is their invariant, enforced there;
    # this lane does not re-assert it, it just satisfies it.
    conn.execute("INSERT INTO capability_registry VALUES ('authority','built','live','2026-08-25')")
    conn.execute("INSERT INTO vocabulary VALUES ('authority','gate','lifecycle')")
    cols = [c.strip() for c in RECORD_COLS.split(",")]
    conn.execute(
        f"INSERT INTO record({RECORD_COLS}) VALUES ({','.join('?' * len(cols))})",
        (
            "r1",
            "authority",
            "gate",
            "a gate",
            "superseded",
            "2026-08-25T00:00:00Z",
            "2026-08-25T00:00:00Z",
            "migrated:janus",
            "g1",
            "",
            "claude-2f",
            "2026-08-25T00:00:00Z",
            "{}",
            "{}",
            "not_required",
            "",
        ),
    )
    return conn


def insert(conn, table, row):
    conn.execute(
        f"INSERT INTO {table}({','.join(row)}) VALUES ({','.join('?' * len(row))})",
        tuple(row.values()),
    )


class TestSeedingActuallyHappened:
    """A loader that applies DDL and skips seeding leaves every lookup table
    empty, so every foreign key rejects everything — loudly, but only at the
    first insert, far from the loader."""

    @pytest.mark.parametrize(
        "table,expected",
        [
            ("enum_replay_outcome", 4),
            ("enum_derivation_kind", 3),
            ("enum_instant_encoding", 4),
            ("enum_digest_algorithm", 2),
        ],
    )
    def test_this_lane_s_vocabularies_are_populated(self, db, table, expected):
        assert len(enum_values(db, table)) == expected

    def test_lane_a_s_vocabularies_are_populated_too(self, db):
        """Their seeding runs in the same loader; if it stopped, everything
        referencing their enums would start failing and it would look like our
        bug."""
        assert enum_values(db, "enum_resolution") == {"resolved", "absent", "broken", "unresolved"}


class TestUnknownVariantsCannotBeStored:
    """Totality check 3 at the storage layer: the database is the reader that
    refuses."""

    @pytest.mark.parametrize("bad", ["maybe", "REPRODUCED", "", "reproduced "])
    def test_an_unknown_replay_outcome_is_refused(self, db, bad):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", _run(outcome=bad))

    def test_an_unknown_instant_encoding_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", _run(observed_encoding="probably_utc"))


def _run(**over):
    row = {
        "id": "run1",
        "subject_record_id": "r1",
        "outcome": "reproduced",
        "environment_digest": "abc",
        "environment_algorithm": "icarus.stableJson.v1",
        "inputs_digest": "def",
        "inputs_algorithm": "icarus.stableJson.v1",
        "observed_at": "2026-08-25T00:00:00Z",
        "observed_encoding": "iso8601z",
    }
    row.update(over)
    return row


class TestDerivedVerdicts:
    """Three kinds; only row-derived may be stored."""

    def _row(self, **over):
        r = {
            "id": "d1",
            "record_id": "r1",
            "field": "attention_reasons",
            "kind": "row_derived",
            "input_fields": '["status","owner"]',
        }
        r.update(over)
        return r

    @pytest.mark.parametrize("kind", ["clock_derived", "cross_table_derived"])
    def test_a_stale_by_construction_verdict_cannot_be_stored(self, db, kind):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "derived_value", self._row(kind=kind, field="claim_state"))

    def test_row_derived_is_storable_because_it_duplicates_rather_than_lies(self, db):
        insert(db, "derived_value", self._row())

    def test_a_derived_value_must_name_its_inputs(self, db):
        """Without them it can be trusted but never recomputed."""
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "derived_value", self._row(input_fields=""))

    def test_all_three_kinds_remain_nameable_even_though_two_are_unstorable(self, db):
        """The vocabulary describes reality; the CHECK decides what may be
        written. Deleting the unstorable kinds would lose the ability to say
        what a value IS."""
        assert enum_values(db, "enum_derivation_kind") == {
            "clock_derived",
            "cross_table_derived",
            "row_derived",
        }


class TestReplayRunsAreHonestAboutWhatTheyDid:
    def test_a_run_that_executed_must_say_what_it_ran_against(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", _run(inputs_digest="", environment_digest=""))

    def test_not_attempted_claims_nothing(self, db):
        insert(
            db,
            "replay_run",
            _run(outcome="not_attempted", environment_digest="", inputs_digest="", observed_at=""),
        )

    def test_not_attempted_with_an_observation_is_refused(self, db):
        """'We did not run it' and 'we ran it at 3pm' cannot both be true."""
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", _run(outcome="not_attempted"))

    def test_a_digest_without_its_algorithm_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", _run(inputs_algorithm=""))

    def test_identity_is_text_never_a_rowid(self, db):
        """record is not WITHOUT ROWID, so an INTEGER FK to its rowid would
        pass every test here and break once, on VACUUM, when rowids renumber."""
        info = {r[1]: r[2] for r in db.execute("PRAGMA table_info(replay_run)")}
        assert info["subject_record_id"] == "TEXT"
        assert info["id"] == "TEXT"
