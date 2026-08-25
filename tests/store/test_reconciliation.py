"""Totality check 2: the schema and the models must claim each other, both ways.

A one-directional check passes for the entire time the schema is the more
permissive of the two — which is exactly the state that produces a silent data
sink, so both directions are asserted here.
"""

from __future__ import annotations

import sqlite3

import pytest

from mosaic.store.ownership import REGISTRY
from mosaic.store.reconcile import (
    Resolution,
    reconcile_columns,
    reconcile_vocabularies,
)
from mosaic.store.schema import apply_schema


@pytest.fixture
def db():
    conn = sqlite3.connect(":memory:")
    apply_schema(conn)
    return conn


class TestEveryVocabularyIsOwned:
    def test_no_lookup_table_is_unowned(self, db):
        unowned = [r for r in reconcile_vocabularies(db) if r.resolution is Resolution.UNOWNED]
        assert not unowned, "\n".join(r.describe() for r in unowned)

    def test_an_unowned_lookup_table_is_actually_detected(self, db):
        """The test above asserts an absence, so it also passes when the
        detector is deleted. This one makes the detector produce something."""
        db.execute("CREATE TABLE rogue_vocab (name TEXT PRIMARY KEY) STRICT")
        db.execute("INSERT INTO rogue_vocab(name) VALUES ('whatever')")
        reports = {r.table: r for r in reconcile_vocabularies(db)}
        assert "rogue_vocab" in reports
        assert reports["rogue_vocab"].resolution is Resolution.UNOWNED
        assert "exactly one owning lane" in reports["rogue_vocab"].describe()

    def test_this_lane_s_own_vocabularies_reconcile(self, db):
        mine = [r for r in reconcile_vocabularies(db) if r.owner == "claude-2f"]
        assert mine, "the registry should own several vocabularies"
        diverged = [r for r in mine if r.resolution is not Resolution.MATCHED]
        assert not diverged, "\n".join(r.describe() for r in diverged)

    def test_a_pending_owner_is_reported_not_skipped(self, db):
        """A silently-skipped check is indistinguishable from a passing one.

        Tests the MECHANISM with a synthetic vocabulary whose owner cannot
        exist, rather than naming a real lane that happens to be late. The
        earlier version asserted `reference_state` was PENDING; it was, until
        claude-09 landed their definition, and then this test failed for the
        good reason that the world had moved. A test whose premise is another
        lane's progress measures their schedule, not this mechanism.
        """
        from mosaic.store import ownership

        ghost = ownership.Vocabulary(
            "enum_resolution", "nobody", "mosaic.store.module_that_will_never_exist", "X"
        )
        assert ownership.canonical_values(ghost) is None

        original = ownership.REGISTRY
        ownership.REGISTRY = tuple(ghost if v.table == "enum_resolution" else v for v in original)
        try:
            report = {r.table: r for r in reconcile_vocabularies(db)}["enum_resolution"]
        finally:
            ownership.REGISTRY = original

        assert report.resolution is Resolution.PENDING
        assert "Not a pass" in report.describe()

    def test_pending_is_distinct_from_matched_and_from_diverged(self):
        assert len({Resolution.PENDING, Resolution.MATCHED, Resolution.DIVERGED}) == 3

    def test_the_registry_holds_addresses_not_values(self):
        """A list of expected values in the registry would BE the second copy."""
        for vocab in REGISTRY:
            assert isinstance(vocab.module, str) and isinstance(vocab.attribute, str)
            assert not hasattr(vocab, "values")


class TestDivergenceIsCaughtInBothDirections:
    def test_a_value_in_sql_that_the_definition_dropped(self, db):
        """The dangerous direction: the schema accepts what no reader handles."""
        db.execute("INSERT INTO enum_replay_outcome(value) VALUES ('probably_fine')")
        report = next(r for r in reconcile_vocabularies(db) if r.table == "enum_replay_outcome")
        assert report.resolution is Resolution.DIVERGED
        assert "probably_fine" in report.only_in_sql
        assert "no reader handles" in report.describe()

    def test_a_value_in_the_definition_that_sql_rejects(self, db):
        """The loud direction — but it still fails at runtime, not in CI."""
        db.execute("DELETE FROM enum_replay_outcome WHERE value = 'not_replayable'")
        report = next(r for r in reconcile_vocabularies(db) if r.table == "enum_replay_outcome")
        assert report.resolution is Resolution.DIVERGED
        assert "not_replayable" in report.only_in_definition
        assert "fail at runtime" in report.describe()

    def test_both_directions_at_once_are_both_reported(self, db):
        db.execute("INSERT INTO enum_replay_outcome(value) VALUES ('extra')")
        db.execute("DELETE FROM enum_replay_outcome WHERE value = 'diverged'")
        report = next(r for r in reconcile_vocabularies(db) if r.table == "enum_replay_outcome")
        assert report.only_in_sql == frozenset({"extra"})
        assert report.only_in_definition == frozenset({"diverged"})


class TestColumnReconciliation:
    def test_a_column_no_model_claims_is_reported(self, db):
        """A silent data sink: writes land and nothing ever reads them."""
        models = {"replay_run": {"id", "subject_record_id", "outcome"}}
        unclaimed, _ = reconcile_columns(db, models)["replay_run"]
        assert "environment_digest" in unclaimed
        assert "inputs_algorithm" in unclaimed

    def test_a_field_no_column_backs_is_reported(self, db):
        models = {"replay_run": {"id", "invented_field"}}
        _, unbacked = reconcile_columns(db, models)["replay_run"]
        assert unbacked == frozenset({"invented_field"})

    def test_a_fully_claimed_table_is_absent_from_the_report(self, db):
        from mosaic.store.schema import schema_columns

        exact = {t: set(c) for t, c in schema_columns(db).items()}
        assert reconcile_columns(db, exact) == {}


class TestTheColumnReconcilerIsActuallyWired:
    """Until this existed, `reconcile_columns` took a model map nobody supplied.

    A reconciler with no caller is inert, and a suite containing one implies
    more coverage than it has. This connects it to claude-09's canonical map,
    which derives from dataclass fields rather than a hand-written list — so a
    divergence here is a real finding, not copy-drift between two hand-lists.
    """

    def test_the_canonical_map_is_importable_and_non_empty(self):
        from mosaic.store.models import CANONICAL_MODELS

        assert CANONICAL_MODELS, "an empty map would make every check below vacuous"

    def test_every_mapped_table_exists_in_the_schema(self, db):
        from mosaic.store.models import CANONICAL_MODELS
        from mosaic.store.schema import schema_columns

        present = set(schema_columns(db))
        missing = {t for t in CANONICAL_MODELS if t not in present}
        assert not missing, (
            f"the model map names tables the schema does not have: {sorted(missing)}. "
            f"Either the migration has not landed or the map is ahead of it."
        )

    def test_no_mapped_table_has_an_unclaimed_column_or_unbacked_field(self, db):
        """Check 2, live. A column no model reads is a silent data sink."""
        from mosaic.store.models import CANONICAL_MODELS, expected_columns

        models = {table: expected_columns(table) for table in CANONICAL_MODELS}
        divergences = reconcile_columns(db, models)
        # Only report on tables the map actually claims; tables owned by other
        # migrations are not this map's business.
        mine = {t: v for t, v in divergences.items() if t in CANONICAL_MODELS}
        assert not mine, "\n".join(
            f"{t}: unclaimed columns {sorted(u)} / unbacked fields {sorted(b)}"
            for t, (u, b) in mine.items()
        )
