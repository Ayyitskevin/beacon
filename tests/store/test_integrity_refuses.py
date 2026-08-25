"""The store must refuse the collapses, not merely document them.

Every case below was found collapsed in live code during the design phase.
A schema that permits them is a schema that will contain them.
"""

from __future__ import annotations

import sqlite3

import pytest

from mosaic.store.schema import apply_schema, enum_values


@pytest.fixture
def db():
    conn = sqlite3.connect(":memory:")
    apply_schema(conn)
    conn.execute(
        "INSERT INTO entity(id,origin,origin_ref,origin_status) VALUES (1,'janus','g1','superseded')"
    )
    return conn


def ref(**over):
    row = {
        "from_entity_id": 1,
        "capability": "athena",
        "ref": "MWS-2",
        "origin": "athena",
        "state": "pending",
        "target_entity_id": None,
        "checked_at": "2026-08-25T00:00:00Z",
        "checked_encoding": "iso8601z",
    }
    row.update(over)
    return row


def insert(conn, table, row):
    cols = ",".join(row)
    marks = ",".join("?" * len(row))
    conn.execute(f"INSERT INTO {table}({cols}) VALUES ({marks})", tuple(row.values()))


class TestUnknownVariantsCannotBeStored:
    """Totality check 3 at the storage layer: the database refuses."""

    @pytest.mark.parametrize("bad", ["maybe", "APPROVED", "", "resolved "])
    def test_an_unknown_reference_state_is_refused(self, db, bad):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "entity_reference", ref(state=bad))

    def test_an_unknown_origin_system_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "entity_reference", ref(capability="replaydeck2"))

    def test_the_enum_is_read_from_the_database_not_hardcoded(self, db):
        """A second copy of an enum is a second source of truth."""
        assert enum_values(db, "reference_state") == {
            "resolved",
            "pending",
            "dangling",
            "unchecked",
        }


class TestNullIsNeverTheSignal:
    def test_resolved_without_a_target_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "entity_reference", ref(state="resolved"))

    def test_a_target_on_an_unresolved_reference_is_refused(self, db):
        """Otherwise a row is silently resolved while claiming to be pending."""
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "entity_reference", ref(state="pending", target_entity_id=1))

    @pytest.mark.parametrize("state", ["resolved", "pending", "dangling"])
    def test_a_claim_about_a_moment_needs_the_moment(self, db, state):
        target = 1 if state == "resolved" else None
        with pytest.raises(sqlite3.IntegrityError):
            insert(
                db,
                "entity_reference",
                ref(state=state, target_entity_id=target, checked_at=None,
                    checked_encoding=None),
            )

    def test_unchecked_is_the_one_state_that_needs_no_moment(self, db):
        """Transient means we did not look — there is no moment to record."""
        insert(db, "entity_reference", ref(state="unchecked", checked_at=None,
                                          checked_encoding=None))

    def test_pending_and_dangling_are_distinct_and_both_storable(self, db):
        """Unresolved is the migration steady state, not a broken reference."""
        insert(db, "entity_reference", ref(state="pending"))
        insert(db, "entity_reference", ref(ref="MWS-3", state="dangling"))
        states = {r[0] for r in db.execute("SELECT state FROM entity_reference")}
        assert states == {"pending", "dangling"}


class TestTheZoneTravelsWithTheInstant:
    def test_an_instant_without_its_encoding_is_refused(self, db):
        """Athena's naive timestamps are UTC only because a default happened to be."""
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "entity_reference", ref(checked_encoding=None))

    def test_an_unknown_encoding_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "entity_reference", ref(checked_encoding="probably_utc"))


class TestDerivedVerdicts:
    """Three kinds; only row-derived may be stored."""

    def row(self, **over):
        r = {
            "entity_id": 1,
            "field": "attention_reasons",
            "kind": "row_derived",
            "inputs_present": 1,
            "input_fields": '["status","owner"]',
        }
        r.update(over)
        return r

    @pytest.mark.parametrize("kind", ["clock_derived", "cross_table_derived"])
    def test_a_stale_by_construction_verdict_cannot_be_stored(self, db, kind):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "derived_value", self.row(kind=kind, field="claim_state"))

    def test_row_derived_is_storable_because_it_duplicates_rather_than_lies(self, db):
        insert(db, "derived_value", self.row())

    def test_a_derived_value_whose_inputs_were_not_migrated_is_refused(self, db):
        """It could then only be trusted, never recomputed."""
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "derived_value", self.row(inputs_present=0, input_fields=None))

    def test_the_three_kinds_are_not_interchangeable(self, db):
        storable = dict(db.execute("SELECT name, storable FROM derivation_kind"))
        assert storable == {
            "clock_derived": 0,
            "cross_table_derived": 0,
            "row_derived": 1,
        }


class TestDigestsCarryTheirAlgorithm:
    """Re-deriving a migrated digest with a new implementation reads as DRIFT."""

    def run(self, **over):
        r = {
            "subject_entity_id": 1,
            "outcome": "reproduced",
            "environment_digest": "abc",
            "environment_digest_algo": "icarus.stableJson.v1",
            "inputs_digest": "def",
            "inputs_digest_algo": "icarus.stableJson.v1",
            "observed_at": "2026-08-25T00:00:00Z",
            "observed_encoding": "iso8601z",
        }
        r.update(over)
        return r

    def test_a_digest_without_its_algorithm_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", self.run(environment_digest_algo=None))

    def test_an_unknown_algorithm_is_refused(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(db, "replay_run", self.run(inputs_digest_algo="python.json.dumps"))

    def test_the_frozen_icarus_canonicaliser_is_registered(self, db):
        names = enum_values(db, "digest_algorithm")
        assert "icarus.stableJson.v1" in names


class TestReplayOutcomeIsFourValued:
    def test_a_run_that_executed_must_name_what_it_ran_against(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            insert(
                db,
                "replay_run",
                {
                    "subject_entity_id": 1,
                    "outcome": "diverged",
                    "observed_at": "2026-08-25T00:00:00Z",
                    "observed_encoding": "iso8601z",
                },
            )

    def test_not_attempted_is_transient_and_records_no_observation(self, db):
        insert(db, "replay_run", {"subject_entity_id": 1, "outcome": "not_attempted"})

    def test_not_attempted_with_an_observation_is_refused(self, db):
        """'We did not run it' and 'we ran it at 3pm' cannot both be true."""
        with pytest.raises(sqlite3.IntegrityError):
            insert(
                db,
                "replay_run",
                {
                    "subject_entity_id": 1,
                    "outcome": "not_attempted",
                    "observed_at": "2026-08-25T00:00:00Z",
                    "observed_encoding": "iso8601z",
                },
            )

    def test_all_four_outcomes_exist_and_are_distinct(self, db):
        assert enum_values(db, "replay_outcome") == {
            "reproduced",
            "diverged",
            "not_replayable",
            "not_attempted",
        }
