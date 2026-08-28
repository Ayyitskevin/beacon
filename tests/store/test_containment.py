"""Every evidence.claim belongs somewhere — enforced where it can be, checked
where it cannot.

The split is the point. Two layers both enforcing an invariant is how they
drift, and a rule described as enforced when it is merely checked is how
someone stops looking.
"""

from __future__ import annotations

import sqlite3

import pytest

from beacon_legacy.store.integrity import (
    MUST_BE_CONTAINED,
    contained_types_present,
    orphaned_claims,
)
from beacon_legacy.store.schema import apply_schema

RECORD_COLS = (
    "id, capability, kind, title, status, occurred_at, recorded_at, "
    "provenance_origin, provenance_source_ref, provenance_source_rev, "
    "provenance_actor, provenance_imported_at, body, body_unmapped, "
    "authority_state, authority_id"
)
TS = "2026-08-25T00:00:00Z"


def add_record(conn, rid, capability="evidence", kind="claim", title="a claim"):
    conn.execute(
        f"INSERT INTO record({RECORD_COLS}) VALUES ({','.join('?' * 16)})",
        (
            rid,
            capability,
            kind,
            title,
            "supported",
            TS,
            TS,
            "migrated:minerva",
            rid,
            "",
            "claude-2f",
            TS,
            "{}",
            "{}",
            "not_required",
            "",
        ),
    )


def add_reference(conn, ref_id, target=None):
    conn.execute(
        "INSERT INTO reference(id, capability, ref, origin, resolution, resolved_to) "
        "VALUES (?,?,?,?,?,?)",
        # The natural key is (capability, ref, origin) and it is UNIQUE, so
        # two references must name two different things. Defaulting the target
        # to the reference id keeps fixtures distinct by construction.
        (ref_id, "evidence", target or ref_id, "migrated:minerva", "resolved", target or ref_id),
    )


def contain(conn, record_id, ref_id):
    conn.execute(
        "INSERT INTO relation(from_record, to_reference, kind) VALUES (?,?,'contains')",
        (record_id, ref_id),
    )


@pytest.fixture
def db():
    conn = sqlite3.connect(":memory:")
    apply_schema(conn)
    conn.execute("INSERT INTO capability_registry VALUES ('evidence','built','live','2026-08-25')")
    conn.execute("INSERT INTO vocabulary VALUES ('evidence','claim','lifecycle')")
    conn.execute("INSERT INTO vocabulary VALUES ('evidence','note','lifecycle')")
    return conn


class TestTheCheckFindsWhatNoConstraintCan:
    """On create the rule cannot be a constraint: the relation references the
    record, so the record must exist first."""

    def test_a_claim_with_no_container_is_reported(self, db):
        add_record(db, "c1")
        orphans = orphaned_claims(db)
        assert [o.record_id for o in orphans] == ["c1"]
        assert "no contains relation" in orphans[0].describe()

    def test_a_contained_claim_is_not_reported(self, db):
        add_record(db, "c1")
        add_reference(db, "ref1")
        contain(db, "c1", "ref1")
        assert orphaned_claims(db) == []

    def test_other_types_are_not_subject_to_the_rule(self, db):
        """MUST_BE_CONTAINED lists the types that bear it, and only those."""
        add_record(db, "n1", kind="note")
        assert orphaned_claims(db) == []

    def test_zero_orphans_over_zero_records_is_distinguishable(self, db):
        """Otherwise 'the rule holds' and 'nothing was checked' read alike."""
        assert orphaned_claims(db) == []
        assert contained_types_present(db) == {("evidence", "claim"): 0}
        add_record(db, "c1")
        assert contained_types_present(db) == {("evidence", "claim"): 1}

    def test_the_rule_names_its_types_as_data(self):
        assert ("evidence", "claim") in MUST_BE_CONTAINED


class TestDeletionIsActuallyEnforced:
    """On delete the database can see the whole mutation, so it refuses."""

    def test_removing_the_only_container_is_refused(self, db):
        add_record(db, "c1")
        add_reference(db, "ref1")
        contain(db, "c1", "ref1")
        with pytest.raises(sqlite3.IntegrityError, match="must belong somewhere"):
            db.execute("DELETE FROM relation WHERE from_record = 'c1'")

    def test_the_claim_still_has_its_container_afterwards(self, db):
        add_record(db, "c1")
        add_reference(db, "ref1")
        contain(db, "c1", "ref1")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("DELETE FROM relation WHERE from_record = 'c1'")
        assert orphaned_claims(db) == []

    def test_a_non_claim_may_lose_its_container(self, db):
        """The rule is per-type; applying it to everything would be a different
        and much larger claim."""
        add_record(db, "n1", kind="note")
        add_reference(db, "ref1")
        contain(db, "n1", "ref1")
        db.execute("DELETE FROM relation WHERE from_record = 'n1'")

    def test_a_non_contains_relation_may_be_removed(self, db):
        """`scopes` is deliberately not containment: access must never follow a
        structural move."""
        add_record(db, "c1")
        add_reference(db, "ref1")
        add_reference(db, "ref2")
        contain(db, "c1", "ref1")
        db.execute(
            "INSERT INTO relation(from_record, to_reference, kind) VALUES ('c1','ref2','scopes')"
        )
        db.execute("DELETE FROM relation WHERE from_record='c1' AND kind='scopes'")
        assert orphaned_claims(db) == []


class TestRetypingIntoAContainedType:
    def test_retyping_an_uncontained_record_to_a_claim_is_refused(self, db):
        add_record(db, "n1", kind="note")
        with pytest.raises(sqlite3.IntegrityError, match="would create an orphan"):
            db.execute("UPDATE record SET kind='claim' WHERE id='n1'")

    def test_retyping_a_contained_record_is_allowed(self, db):
        add_record(db, "n1", kind="note")
        add_reference(db, "ref1")
        contain(db, "n1", "ref1")
        db.execute("UPDATE record SET kind='claim' WHERE id='n1'")
        assert orphaned_claims(db) == []


class TestWeDoNotReEnforceLaneAsInvariant:
    def test_at_most_one_container_is_lane_a_s_unique_index(self, db):
        """Asserted here as *theirs*, so a reader knows where it lives — not
        re-enforced, which is how two layers drift apart."""
        add_record(db, "c1")
        add_reference(db, "ref1")
        add_reference(db, "ref2")
        contain(db, "c1", "ref1")
        with pytest.raises(sqlite3.IntegrityError):
            contain(db, "c1", "ref2")
