"""The schema's constraints, proved by execution against a real database.

Lane C owns integrity DDL. These tests assert the constraints Lane A declares
are *actually enforced* rather than re-enforcing them -- the split agreed for
this phase: each invariant lives in exactly one layer, and the other layer
tests that it is there. Two layers both enforcing is how they drift.

Every test here fails if the constraint is removed. A schema test that only
inserts valid rows proves nothing: it passes against a schema with no
constraints at all.
"""

from __future__ import annotations

import pathlib
import sqlite3

import pytest

from mosaic.store.enums import (
    CANONICAL_ENUMS,
    divergences,
    seed_capability_registry,
    seed_enums,
)
from mosaic.store.models import CANONICAL_MODELS, column_divergences, expected_columns

SCHEMA = pathlib.Path(__file__).resolve().parents[2] / "src" / "mosaic" / "store" / "schema.sql"


@pytest.fixture
def db() -> sqlite3.Connection:
    conn = sqlite3.connect(":memory:")
    conn.executescript(SCHEMA.read_text())
    seed_enums(conn)
    seed_capability_registry(conn)
    conn.execute("INSERT INTO vocabulary VALUES ('authority','gate','lifecycle')")
    return conn


def _authority_insert(**over: str) -> tuple[str, tuple]:
    cols = dict(
        id="a1",
        decider="human",
        check_result="verified",
        enforcement="reported",
        lifecycle="authorized",
        force="in_force",
        binding_kind="none",
        mechanism="janus.ruling",
        meaning="a human ruled on these bytes",
    )
    cols.update(over)
    return (
        f"INSERT INTO authority ({','.join(cols)}) VALUES ({','.join('?' * len(cols))})",
        tuple(cols.values()),
    )


class TestEnumsHaveOneCanonicalSource:
    """Inline CHECK lists were a second hand-maintained copy. These assert the
    lookup tables are materialized from the canonical definitions instead."""

    def test_seeded_database_matches_the_definitions(self, db):
        assert divergences(db) == {}

    def test_divergence_is_detected_in_the_silent_direction(self, db):
        """The direction that matters and nobody notices: a value in the
        database that the code has dropped keeps validating forever, because
        nothing ever tries to store it and fail."""
        db.execute("INSERT INTO enum_force (value) VALUES ('legacy_ghost')")
        assert divergences(db)["enum_force"]["in_db_not_code"] == ["legacy_ghost"]

    def test_divergence_is_detected_in_the_loud_direction(self, db):
        db.execute("DELETE FROM enum_force WHERE value = 'spent'")
        assert divergences(db)["enum_force"]["in_code_not_db"] == ["spent"]

    def test_every_enum_table_has_a_canonical_owner(self, db):
        """An enum table with no entry in CANONICAL_ENUMS has no owner, which
        is the state this rule exists to prevent."""
        tables = {
            r[0]
            for r in db.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'enum_%'"
            )
        }
        assert tables == set(CANONICAL_ENUMS)


class TestModelMapMatchesTheSchema:
    """Totality check 2, wired to Lane C's reconciler.

    Check 1 (a field added to a type) cannot see a column added to the schema
    that no type models. This is that direction.
    """

    def test_no_column_divergence(self, db):
        assert column_divergences(db) == {}

    def test_divergence_is_detected_in_the_silent_direction(self, db):
        """A column no model knows about: nothing fails, the schema simply
        carries a field nothing maintains, and it accumulates."""
        db.execute("ALTER TABLE record ADD COLUMN orphan TEXT NOT NULL DEFAULT ''")
        assert column_divergences(db)["record"]["in_db_not_model"] == ["orphan"]

    def test_expected_columns_is_derived_not_listed(self, db):
        """If this were a hand-written list it would be a second copy of the
        model, free to drift -- the defect the enum lookup tables removed."""
        assert "provenance_origin" in expected_columns("record")
        assert "provenance" not in expected_columns("record")

    def test_every_non_enum_table_has_a_canonical_model_or_is_declared(self, db):
        """An unmapped table has no canonical owner. Listed explicitly so
        adding a table forces a decision rather than silently opting out."""
        known_unmapped = {
            "authority",
            "vocabulary",
            "vocabulary_member",
            "capability_registry",
            "binding_digest",
            "binding_intent",
            "binding_handle",
            "binding_step",
            "binding_none",
        }
        tables = {
            r[0]
            for r in db.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'enum_%'"
            )
        }
        assert tables - set(CANONICAL_MODELS) == known_unmapped


class TestSchemaLoads:
    def test_every_table_exists(self, db):
        got = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        assert {
            "record",
            "reference",
            "relation",
            "authority",
            "vocabulary",
            "vocabulary_member",
            "capability_registry",
            "binding_digest",
            "binding_intent",
            "binding_handle",
            "binding_step",
            "binding_none",
        } <= got


class TestNoDefaultsOnMeaningfulColumns:
    """A default is a value nobody chose."""

    def test_force_cannot_be_omitted(self, db):
        """The regression this pins by name: `spent BOOLEAN DEFAULT 0` held
        three facts in two states and defaulted to the wrong one, which is why
        five superseded gates reported as live."""
        sql, params = _authority_insert()
        cols = [c for c in sql.split("(")[1].split(")")[0].split(",") if c != "force"]
        vals = [
            v
            for c, v in zip(sql.split("(")[1].split(")")[0].split(","), params, strict=True)
            if c != "force"
        ]
        with pytest.raises(sqlite3.IntegrityError):
            db.execute(
                f"INSERT INTO authority ({','.join(cols)}) VALUES ({','.join('?' * len(cols))})",
                tuple(vals),
            )

    def test_valid_authority_inserts(self, db):
        """Guard against a schema so strict nothing works -- the mutation that
        would make every other test in this class pass for the wrong reason."""
        sql, params = _authority_insert()
        db.execute(sql, params)

    @pytest.mark.parametrize("bad", ["approved", "spent_", "", "TRUE"])
    def test_force_rejects_values_outside_the_vocabulary(self, db, bad):
        """Enforced by a foreign key to the seeded lookup table, not a CHECK."""
        sql, params = _authority_insert(force=bad)
        with pytest.raises(sqlite3.IntegrityError):
            db.execute(sql, params)

    def test_lifecycle_has_the_terminal_member(self, db):
        """The member the mandated five lacked, which is what forced `spent`
        into existence as a workaround."""
        sql, params = _authority_insert(lifecycle="closed_negative")
        db.execute(sql, params)


class TestResolutionIsNotDestructive:
    """The triple is retained; resolved_to is recorded alongside, never in
    place of it. A wrong resolution that has eaten its own input is
    undetectable forever."""

    def test_resolved_must_name_what_it_resolved_to(self, db):
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO reference VALUES ('r','authority','g1','native','resolved','')")

    @pytest.mark.parametrize("state", ["absent", "broken", "unresolved"])
    def test_unresolved_must_not_carry_a_stale_conclusion(self, db, state):
        with pytest.raises(sqlite3.IntegrityError):
            db.execute(
                "INSERT INTO reference VALUES (?,?,?,?,?,?)",
                ("r", "authority", "g1", "native", state, "abc123"),
            )

    def test_absent_is_a_normal_storable_state(self, db):
        """`absent` is the common, healthy case -- we know what it names and do
        not hold it. A foreign key that refused it would delete the "2 of 7
        trace hops joinable" measurement."""
        db.execute("INSERT INTO reference VALUES ('r','evaluation','RD-1','native','absent','')")


class TestCapabilityRegistry:
    """Level 4: a registry derived from what exists cannot report what does not."""

    def test_the_registry_is_seeded_not_empty(self, db):
        """A registry with no rows reports nothing missing -- the exact failure
        level 4 exists to prevent. It shipped empty until claude-0d noticed."""
        assert db.execute("SELECT count(*) FROM capability_registry").fetchone()[0] == 7

    def test_capabilities_that_do_not_exist_have_rows(self, db):
        missing = {
            r[0]
            for r in db.execute("SELECT capability FROM capability_registry WHERE state != 'built'")
        }
        assert {"evaluation", "conversation", "execution"} <= missing

    def test_never_built_and_deferred_are_distinguished(self, db):
        """Both are "no implementation" and they are different facts: reporting
        a deferred capability as unbuilt sends someone looking for code that
        exists."""
        states = dict(db.execute("SELECT capability, state FROM capability_registry"))
        assert states["evaluation"] == "not_built"
        assert states["execution"] == "deferred"

    def test_reason_is_required(self, db):
        """'never built' and 'deferred, no run state on this host' are both
        "no implementation" and are different facts. Reporting the first as
        the second sends someone to restart a service nobody wrote."""
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO capability_registry VALUES ('execution','not_built','',NULL)")


class TestStatusPolarity:
    """`is_terminal_negative: bool` held two absences -- claude-0d showed `= 0`
    returning open, in_progress, done AND verified."""

    def test_polarity_separates_not_finished_from_finished_well(self, db):
        db.execute("INSERT INTO vocabulary VALUES ('work','issue','lifecycle')")
        for word, pol in [("open", "active"), ("done", "terminal_positive")]:
            db.execute("INSERT INTO vocabulary_member VALUES ('work','issue',?,?)", (word, pol))
        got = dict(db.execute("SELECT member, polarity FROM vocabulary_member WHERE kind='issue'"))
        assert got["open"] != got["done"]

    def test_refused_and_superseded_are_distinguished(self, db):
        """A refusal is a decision; expired/withdrawn/superseded are the absence
        of one. Janus's own supersede prints "NOBODY RULED"."""
        for word, pol in [("refused", "terminal_negative"), ("superseded", "terminal_unruled")]:
            db.execute("INSERT INTO vocabulary_member VALUES ('authority','gate',?,?)", (word, pol))
        got = dict(db.execute("SELECT member, polarity FROM vocabulary_member WHERE kind='gate'"))
        assert got["refused"] != got["superseded"]

    def test_a_boolean_polarity_is_refused(self, db):
        """The legacy shape must not be storable."""
        db.execute("INSERT INTO vocabulary VALUES ('work','issue','lifecycle')")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO vocabulary_member VALUES ('work','issue','open',0)")


class TestRelationsAreTyped:
    @pytest.fixture
    def seeded(self, db):
        db.execute("INSERT INTO reference VALUES ('r1','authority','g1','native','absent','')")
        db.execute("INSERT INTO reference VALUES ('r2','authority','g2','native','absent','')")
        sql, params = _authority_insert()
        db.execute(sql, params)
        db.execute(
            "INSERT INTO record VALUES ('rec1','authority','gate','t','proposed','t0','t1',"
            "'native','','','actor','','{}','', 'required','a1')"
        )
        return db

    def test_containment_is_singular(self, seeded):
        seeded.execute("INSERT INTO relation VALUES ('rec1','r1','contains')")
        with pytest.raises(sqlite3.IntegrityError):
            seeded.execute("INSERT INTO relation VALUES ('rec1','r2','contains')")

    def test_scope_is_not_singular_and_not_inherited(self, seeded):
        """Scope is deliberately unconstrained to one and deliberately not
        derived from containment: a structural move must never silently change
        who can see a record."""
        seeded.execute("INSERT INTO relation VALUES ('rec1','r1','scopes')")
        seeded.execute("INSERT INTO relation VALUES ('rec1','r2','scopes')")

    def test_reply_may_cross_containment(self, seeded):
        seeded.execute("INSERT INTO relation VALUES ('rec1','r1','contains')")
        seeded.execute("INSERT INTO relation VALUES ('rec1','r2','replies_to')")

    def test_unknown_relation_kind_is_refused(self, seeded):
        with pytest.raises(sqlite3.IntegrityError):
            seeded.execute("INSERT INTO relation VALUES ('rec1','r1','parent')")


class TestBindingsAreATaggedUnion:
    """Separate tables per variant, not one table with nullable columns."""

    def test_digest_requires_its_algorithm(self, db):
        """Re-deriving a migrated Icarus digest with a different canonicaliser
        does not look like a tooling change -- it looks like drift, fired
        across every migrated approval at once."""
        sql, params = _authority_insert(binding_kind="digest")
        db.execute(sql, params)
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO binding_digest VALUES ('a1','abc','','plan')")

    def test_digest_with_algorithm_inserts(self, db):
        sql, params = _authority_insert(binding_kind="digest")
        db.execute(sql, params)
        db.execute("INSERT INTO binding_digest VALUES ('a1','abc','icarus.stableJson.v1','plan')")

    def test_no_binding_requires_a_reason(self, db):
        """An unexplained absence is indistinguishable from an oversight."""
        sql, params = _authority_insert()
        db.execute(sql, params)
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO binding_none VALUES ('a1','')")


class TestAuthorityStateIsExplicit:
    """ "no approval was sought" and "an approval is missing" are different
    facts; a nullable FK would make them one."""

    def test_not_required_must_not_carry_an_authority(self, db):
        sql, params = _authority_insert()
        db.execute(sql, params)
        with pytest.raises(sqlite3.IntegrityError):
            db.execute(
                "INSERT INTO record VALUES ('rec','authority','gate','t','proposed','t0','t1',"
                "'native','','','actor','','{}','', 'not_required','a1')"
            )
